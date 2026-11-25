//! The core's screens as Kotlin sees them (`zcore::screen`, `zcore::root`): one root per app, owning every screen
//! object the app sends events to, whose view model it draws and whose effects it runs. The types are zcore's own,
//! described to UniFFI here (`remote`), so a field added in zcore that isn't added here fails to build instead of going
//! missing. Some are renamed for Kotlin, where OkHttp's `Request` and the app's own `Device` and `Route` already are.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use zcore::history::{Day as HistoryDay, Event as HistoryEvent, Row as HistoryRow, View as HistoryView};
use zcore::api::Failure as ApiFailure;
use zcore::on_tv::Order as TvOrder;
use zcore::root::{Delivery, Order, Root, ScreenId, Turn};
use zcore::play::Command as PlayCommand;
use zcore::store::{Need as PageNeed, PageReply};
use zcore::screen::{
    AppEvent, Body as HttpBody, Clock as CoreClock, Device as DeviceKind, DiskEntry, DiskOp, DiskReply, Effect, Empty as EmptyState,
    Glyph as Lucide, Header as HttpHeader, Method as HttpMethod, Platform, Reply, Request as HttpRequest,
    Route as CoreRoute, Screen, Trouble as TroubleState,
};

#[uniffi::remote(Enum)]
pub enum Effect {
    Request { id: u32, request: HttpRequest },
    Fetch { id: u32, request: HttpRequest, file: String },
    Page { id: u32, path: String, key: String, need: PageNeed },
    Timer { id: u32, ms: u32 },
    Cancel { id: u32 },
    ReadPref { id: u32, key: String },
    WritePref { key: String, value: Option<String> },
    Load { id: u32, key: String },
    Store { key: String, body: String, etag: Option<String> },
    Play { show: String, episode: Option<String> },
    Warm { show: String, episode: Option<String> },
    Open { route: CoreRoute },
    SignedOut,
    Profile { id: String },
    Forget,
    Tell { event: AppEvent },
    Report { place: String, message: String },
    Player { order: PlayCommand },
    Tv { order: TvOrder },
    Disk { id: u32, op: DiskOp },
}

#[uniffi::remote(Enum)]
pub enum Reply {
    Response { status: u16, headers: Vec<HttpHeader>, body: String },
    Saved { status: u16, bytes: u64 },
    Unreachable { offline: bool, message: String },
    Fired,
    Pref { value: Option<String> },
    Page { page: PageReply },
    Kept { body: String, etag: Option<String>, at: f64 },
    Missing,
    Disk { disk: DiskReply },
}

#[uniffi::remote(Enum)]
pub enum DiskOp {
    Read { path: String },
    Write { path: String, text: String },
    Append { from: String, to: String, at: u64 },
    Rename { from: String, to: String },
    Remove { paths: Vec<String> },
    Listing { path: String },
    Sizes { paths: Vec<String> },
    Picture { path: String },
    Free,
    Hash { path: String },
}

#[uniffi::remote(Record)]
pub struct DiskReply {
    pub ok: bool,
    pub message: String,
    pub text: Option<String>,
    pub entries: Vec<DiskEntry>,
    pub bytes: u64,
}

#[uniffi::remote(Record)]
pub struct DiskEntry {
    pub name: String,
    pub bytes: u64,
}

#[uniffi::remote(Record)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<HttpHeader>,
    pub body: Option<HttpBody>,
    pub cookies: bool,
    pub timeout_ms: u32,
}

#[uniffi::remote(Enum)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

#[uniffi::remote(Record)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

#[uniffi::remote(Enum)]
pub enum HttpBody {
    Text { text: String, content_type: String },
    Bytes { bytes: Vec<u8>, content_type: String },
}

#[uniffi::remote(Enum)]
pub enum CoreRoute {
    Title { id: String },
    Section { key: String },
    Search,
    History,
    Settings,
    Downloads,
    YourTv,
    Manhwa,
    Other { name: String, arg: String },
}

#[uniffi::remote(Enum)]
pub enum DeviceKind {
    Phone,
    Tv,
    Desktop,
}

#[uniffi::remote(Record)]
pub struct Platform {
    pub device: DeviceKind,
    pub model: String,
    pub os: String,
    pub version_name: String,
    pub version_code: i64,
    pub stream: String,
    pub codecs: Vec<String>,
    pub hdr: bool,
}

#[uniffi::remote(Record)]
pub struct CoreClock {
    pub now: f64,
    pub utc_offset: i32,
}

#[uniffi::remote(Enum)]
pub enum AppEvent {
    Lifecycle { foreground: bool },
    Connectivity { online: bool, metered: bool },
    PageChanged { key: String },
    ProfilesChanged,
    ProfileGone { id: String },
    Account { account: Option<CoreAccount> },
    Watched,
    Played { show: String, episode: Option<String> },
    DownloadSettings { quality: String, anime_audio: String },
    Maintenance { message: String },
}

#[uniffi::remote(Record)]
pub struct Order {
    pub screen: u32,
    pub effect: Effect,
}

#[uniffi::remote(Record)]
pub struct Turn {
    pub changed: Vec<u32>,
    pub orders: Vec<Order>,
    pub delivered: Vec<Delivery>,
    pub pages_changed: Vec<String>,
}

#[uniffi::remote(Enum)]
pub enum PageNeed {
    Any,
    Current,
    Server,
}

#[uniffi::remote(Record)]
pub struct Delivery {
    pub ticket: u64,
    pub page: PageReply,
}

#[uniffi::remote(Record)]
pub struct PageReply {
    pub body: Option<String>,
    pub at: f64,
    pub failure: Option<ApiFailure>,
    pub stale: bool,
}

#[uniffi::remote(Record)]
pub struct TroubleState {
    pub message: String,
    pub offline: bool,
    pub busy: bool,
}

#[uniffi::remote(Record)]
pub struct EmptyState {
    pub glyph: Lucide,
    pub title: String,
    pub line: String,
}

#[uniffi::remote(Enum)]
pub enum Lucide {
    History,
    Search,
    Download,
    House,
    Clapperboard,
    Tv,
    Film,
    Sparkles,
    BookOpen,
}

/// The app's one root (`zcore::root::Root`): the screens live on it, and every answer and app-wide event goes to it.
/// Behind a lock, as UniFFI objects are shared across threads; the app calls it from one worker thread.
#[derive(uniffi::Object)]
pub struct CoreRoot(Mutex<Root>);

#[uniffi::export]
impl CoreRoot {
    /// The app starting on the device `platform`, talking to the server at `base`.
    #[uniffi::constructor]
    pub fn new(platform: Platform, base: String, clock: CoreClock) -> Self {
        Self(Mutex::new(Root::new(platform, base, clock)))
    }

    /// The answer to effect `id`.
    pub fn answer(&self, id: u32, reply: Reply, clock: CoreClock) -> Turn {
        self.lock().answer(id, reply, clock)
    }

    /// Something happened to the whole app (the background, the network).
    pub fn app(&self, event: AppEvent, clock: CoreClock) -> Turn {
        self.lock().app(event, clock)
    }

    /// The app's own code wants the server page at `path`, kept under `key`; it comes back in a `Turn`'s `delivered`
    /// under `ticket`.
    pub fn page(&self, path: String, key: String, need: PageNeed, ticket: u64, clock: CoreClock) -> Turn {
        self.lock().page(&path, &key, need, ticket, clock)
    }

    /// The screen `screen` has gone: what it was waiting for is cancelled.
    pub fn close_screen(&self, screen: u32, clock: CoreClock) -> Turn {
        self.lock().close(screen, clock)
    }
}

impl CoreRoot {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Root> {
        // (every call leaves the root whole, so a panic elsewhere can't have left it half-changed)
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A screen object: the core's model, live on the root from its making until the root closes it.
macro_rules! screen {
    ($(#[$doc:meta])* $name:ident($model:ty), $event:ty, $view:ty) => {
        $(#[$doc])*
        #[derive(uniffi::Object)]
        pub struct $name {
            root: Arc<CoreRoot>,
            id: ScreenId,
        }

        #[uniffi::export]
        impl $name {
            #[uniffi::constructor]
            pub fn new(root: Arc<CoreRoot>) -> Self {
                let id = root.lock().open(<$model>::default());
                Self { root, id }
            }

            /// Its id on the root, as `Turn`s and `CoreRoot.close_screen` name it.
            pub fn id(&self) -> u32 {
                self.id
            }

            /// Something happened on the screen. Draw `view()` again for each changed screen.
            pub fn update(&self, event: $event, clock: CoreClock) -> Turn {
                self.root.lock().update::<$model>(self.id, event, clock)
            }

            /// Exactly what to draw.
            pub fn view(&self) -> $view {
                self.root.lock().view::<$model>(self.id).unwrap_or_else(|| <$model>::default().view())
            }
        }

        impl Drop for $name {
            fn drop(&mut self) {
                // (closed on the root already, normally: this only makes sure nothing is left waiting for it)
                self.root.lock().close(self.id, CoreClock::default());
            }
        }
    };
}
pub(crate) use screen;

#[uniffi::remote(Enum)]
pub enum HistoryEvent {
    Opened { profile: String },
    Changed,
    Query { text: String },
    Tapped { key: String },
    Retry,
}

#[uniffi::remote(Record)]
pub struct HistoryView {
    pub title: String,
    pub hint: String,
    pub count: Option<String>,
    pub loading: bool,
    pub days: Vec<HistoryDay>,
    pub empty: Option<EmptyState>,
    pub trouble: Option<TroubleState>,
}

#[uniffi::remote(Record)]
pub struct HistoryDay {
    pub heading: String,
    pub rows: Vec<HistoryRow>,
}

#[uniffi::remote(Record)]
pub struct HistoryRow {
    pub key: String,
    pub title: String,
    pub line: String,
    pub picture: Option<String>,
    pub progress: Option<f32>,
}

screen!(
    /// Watch history (`zcore::history`).
    HistoryScreen(zcore::history::History),
    HistoryEvent,
    HistoryView
);

use zcore::session::{
    Account as CoreAccount, Event as SessionEvent, Field as SignInField, Mode as SignInMode, Profile as CoreProfile, SignInView,
    Stage as SessionStage, View as SessionView,
};

#[uniffi::remote(Record)]
pub struct CoreProfile {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    pub index: u32,
    pub settings: Option<String>,
}

#[uniffi::remote(Record)]
pub struct CoreAccount {
    pub profiles: Vec<CoreProfile>,
    pub current: Option<String>,
    pub plus: bool,
    pub max_profiles: u32,
}

#[uniffi::remote(Enum)]
pub enum SessionStage {
    Splash,
    SignIn,
    Waiting,
    Who,
    Main,
    Maintenance,
}

#[uniffi::remote(Enum)]
pub enum SignInMode {
    Phone,
    Create,
    LogIn,
    Code,
    Forgot,
    Reset,
}

#[uniffi::remote(Enum)]
pub enum SignInField {
    Email,
    Code,
    Username,
    Password,
}

#[uniffi::remote(Record)]
pub struct SignInView {
    pub mode: SignInMode,
    pub fields: Vec<SignInField>,
    pub tabs: bool,
    pub forgot: bool,
    pub resend: bool,
    pub back: Option<SignInMode>,
    pub phone_tab: bool,
    pub line: String,
    pub button: String,
    pub busy: bool,
    pub error: Option<String>,
    pub shakes: u32,
    pub remember: Option<bool>,
    pub phone_tab_title: String,
    pub create_tab_title: String,
    pub log_in_tab_title: String,
    pub remember_title: String,
    pub email_title: String,
    pub username_title: String,
    pub password_title: String,
    pub code_title: String,
    pub forgot_title: String,
    pub back_title: String,
    pub resend_title: String,
    pub resend_ready: bool,
    pub toast: Option<String>,
    pub toasts: u32,
}

#[uniffi::remote(Record)]
pub struct SessionView {
    pub stage: SessionStage,
    pub offline: bool,
    pub sign_in: SignInView,
    pub wall: Vec<String>,
    pub waiting_title: String,
    pub waiting_line: String,
    pub waiting_sign_out: String,
    pub maintenance_title: String,
    pub maintenance_message: String,
    pub maintenance_back: String,
    pub maintenance_over: bool,
    pub maintenance_continue: String,
    pub who_title: String,
    pub picking: Option<String>,
    pub who_note: Option<String>,
    pub who_mark: bool,
    pub account: Option<CoreAccount>,
    pub who_manage: String,
    pub who_done: String,
    pub who_add: String,
    pub who_heading: String,
    pub can_manage: bool,
    pub can_add: bool,
    pub managing: bool,
    pub editing: Option<String>,
}

#[uniffi::remote(Enum)]
pub enum SessionEvent {
    Started,
    SplashShown { held: bool },
    SplashDone,
    Mode { mode: SignInMode },
    Submit { email: String, username: String, password: String, code: String, remember: bool },
    Resend,
    SignedInElsewhere,
    Pick { id: String },
    SignOut,
    SignedOut,
    Manage { on: bool },
    Add,
    EditClosed,
    Continue,
}

screen!(
    /// The app from its first frame to a profile in use (`zcore::session`).
    SessionScreen(zcore::session::Session),
    SessionEvent,
    SessionView
);

/// Where Home's lists are kept (`zcore::home::KEY`).
#[uniffi::export]
pub fn home_key() -> String {
    zcore::home::KEY.into()
}

use zcore::profiles::{
    Card as ProfileTileCard, EditEvent as ProfileEditEvent, EditView as ProfileEditView, EditWords as ProfileEditWords,
    Picture as AvatarPicture, Tile as ProfilesTile,
};

#[uniffi::remote(Record)]
pub struct ProfileTileCard {
    pub profile: CoreProfile,
    pub role: String,
    pub deletable: bool,
}

#[uniffi::remote(Record)]
pub struct ProfilesTile {
    pub title: String,
    pub line: String,
    pub cards: Vec<ProfileTileCard>,
    pub manages: bool,
    pub add: Option<String>,
    pub add_title: String,
    pub edit: String,
}

#[uniffi::remote(Record)]
pub struct AvatarPicture {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub crop: Option<String>,
}

#[uniffi::remote(Record)]
pub struct ProfileEditView {
    pub title: String,
    pub button: String,
    pub busy: bool,
    pub note: Option<String>,
    pub delete: Option<bool>,
    pub removing: bool,
    pub removed: bool,
    pub can_remove: bool,
    pub cancel: bool,
    pub saved: bool,
    pub closed: u32,
    pub words: ProfileEditWords,
}

#[uniffi::remote(Record)]
pub struct ProfileEditWords {
    pub name: String,
    pub name_hint: String,
    pub choose_another: String,
    pub remove_picture: String,
    pub remove_ask: String,
    pub delete: String,
    pub delete_ask: String,
    pub cancel: String,
}

#[uniffi::remote(Enum)]
pub enum ProfileEditEvent {
    Opened { profile: Option<CoreProfile>, account: CoreAccount },
    Picked,
    PickFailed { words: String },
    Remove,
    Save { name: String, picture: Option<AvatarPicture> },
    Delete,
}

screen!(
    /// A profile's edit card (`zcore::profiles::Editor`).
    ProfileEditorScreen(zcore::profiles::Editor),
    ProfileEditEvent,
    ProfileEditView
);

/// Settings' profiles tile for the account (`zcore::profiles::tile`).
#[uniffi::export]
pub fn profiles_tile(account: CoreAccount) -> ProfilesTile {
    zcore::profiles::tile(&account)
}

/// How a profile picture is read and sent (`zcore::profiles`).
#[derive(Debug, uniffi::Record)]
pub struct AvatarRules {
    pub name_most: u32,
    pub picture_most_bytes: u64,
    pub side: u32,
    pub most_bytes: u64,
    pub qualities: Vec<u8>,
    pub crop_most: f32,
    pub unreadable: String,
    pub too_big: String,
    pub no_picker: String,
}

/// A profile's name as typed, cut to what a name may hold (`zcore::profiles::clamp_name`).
#[uniffi::export]
pub fn profile_name_clamp(typed: String) -> String {
    zcore::profiles::clamp_name(&typed)
}

/// The letter a profile without a picture shows (`zcore::profiles::initial`).
#[uniffi::export]
pub fn profile_initial(name: String) -> String {
    zcore::profiles::initial(&name)
}

#[uniffi::export]
pub fn avatar_rules() -> AvatarRules {
    use zcore::profiles as p;
    AvatarRules {
        name_most: p::NAME_MOST as u32,
        picture_most_bytes: p::PICTURE_MOST_BYTES as u64,
        side: p::AVATAR_SIDE,
        most_bytes: p::AVATAR_MOST_BYTES as u64,
        qualities: p::AVATAR_QUALITIES.to_vec(),
        crop_most: p::CROP_MOST,
        unreadable: p::UNREADABLE.into(),
        too_big: p::TOO_BIG.into(),
        no_picker: p::NO_PICKER.into(),
    }
}

/// A GIF's square, sent with it whole (`zcore::profiles::crop_query`).
#[uniffi::export]
pub fn avatar_crop_query(x: u32, y: u32, side: u32) -> String {
    zcore::profiles::crop_query(x, y, side)
}

/// How many times smaller to read a picture for the cropper (`zcore::profiles::read_sample`).
#[uniffi::export]
pub fn avatar_read_sample(width: u32, height: u32) -> u32 {
    zcore::profiles::read_sample(width, height)
}

/// Whether a picture's first bytes are a GIF's (`zcore::profiles::is_gif`).
#[uniffi::export]
pub fn avatar_is_gif(head: Vec<u8>) -> bool {
    zcore::profiles::is_gif(&head)
}

/// Whether picking a profile in the menu switches to it (`zcore::profiles::switches`).
#[uniffi::export]
pub fn profile_switches(current: Option<String>, picked: String) -> bool {
    zcore::profiles::switches(current.as_deref(), &picked)
}
