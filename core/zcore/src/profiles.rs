//! An account's profiles, as Settings shows them and the edit card changes them: who may manage which, how many there
//! may be, the words, and adding, renaming, picturing and deleting one. The apps read and crop the picture itself
//! (by the rules here) and draw the card.

use crate::api::{self, Outcome};
use crate::screen::{AppEvent, Asks, Body, Context, Device, Effect, Reply, Screen};
use crate::session::{Account, Profile, PROFILES};
use serde_json::{json, Value};

/// The longest a name may be.
pub const NAME_MOST: usize = 24;
/// The largest picture read for the cropper.
pub const PICTURE_MOST_BYTES: usize = 10 * 1024 * 1024;
/// A still is sent as a JPEG of its square at most this many pixels across...
pub const AVATAR_SIDE: u32 = 256;
/// ...and this small, trying these qualities in turn.
pub const AVATAR_MOST_BYTES: usize = 60 * 1024;
pub const AVATAR_QUALITIES: [u8; 6] = [82, 74, 66, 58, 48, 35];
/// How far the cropper zooms in.
pub const CROP_MOST: f32 = 4.;

pub const UNREADABLE: &str = "That picture couldn't be opened.";
pub const TOO_BIG: &str = "That picture is too big.";

/// Settings' profiles tile: the profiles shown, and whether this one manages them.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub title: String,
    pub line: String,
    pub cards: Vec<Card>,
    /// This profile manages them all (the main one): Manage profiles shows.
    pub manages: bool,
    /// "Add profile" shows, with how many there may be ("Up to 5"), when this profile manages them and there's room.
    pub add: Option<String>,
    /// The add card's name.
    pub add_title: String,
    /// A card's edit button.
    pub edit: String,
}

/// A profile on the tile.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub profile: Profile,
    /// "You", "Main profile" or "Profile".
    pub role: String,
    /// It may be deleted from its card.
    pub deletable: bool,
}

/// The tile for `account`: the main profile (the account's first) manages them all; any other only itself.
pub fn tile(account: &Account) -> Tile {
    let me = account.current.as_deref();
    let main = account.me().is_some_and(|m| m.index == 0);
    let cards = account
        .profiles
        .iter()
        .filter(|p| main || Some(p.id.as_str()) == me)
        .map(|p| Card {
            role: if Some(p.id.as_str()) == me { "You" } else if p.index == 0 { "Main profile" } else { "Profile" }.into(),
            deletable: main && p.index != 0,
            profile: p.clone(),
        })
        .collect();
    let room = (account.profiles.len() as u64) < u64::from(account.max_profiles);
    let add = (main && room).then(|| {
        if account.max_profiles < u32::MAX { format!("Up to {}", account.max_profiles) } else { String::new() }
    });
    Tile {
        title: if main { "Profiles" } else { "Your profile" }.into(),
        line: if main {
            "You're the main profile: manage every profile on this account."
        } else {
            "Your name. The main profile manages the rest."
        }
        .into(),
        cards,
        manages: main,
        add,
        add_title: "Add profile".into(),
        edit: "Edit".into(),
    }
}

/// Where a picture's square is said for a GIF, sent whole: "?crop=x,y,side" in its own pixels.
pub fn crop_query(x: u32, y: u32, side: u32) -> String {
    format!("?crop={x},{y},{side}")
}

/// The cropper over a picture `width` by `height` (its pixels as read): the middle of the square shown, and how far in
/// (1: the whole middle square). What it shows is exactly what's saved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub width: f32,
    pub height: f32,
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

/// The square a [`Crop`] shows, in whole pixels, inside the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Square {
    pub x: u32,
    pub y: u32,
    pub side: u32,
}

impl Crop {
    /// A picture just picked, framed on its whole middle square.
    pub fn new(width: f32, height: f32) -> Self {
        let (width, height) = (width.max(1.), height.max(1.));
        Self { width, height, x: width / 2., y: height / 2., zoom: 1. }
    }

    fn short(&self) -> f32 {
        self.width.min(self.height)
    }

    /// The square's side, in the picture's pixels.
    pub fn side(&self) -> f32 {
        self.short() / self.zoom.max(1.)
    }

    /// Kept inside the picture: never past its edges.
    fn clamped(mut self) -> Self {
        self.zoom = self.zoom.clamp(1., CROP_MOST);
        let half = self.side() / 2.;
        self.x = self.x.clamp(half, (self.width - half).max(half));
        self.y = self.y.clamp(half, (self.height - half).max(half));
        self
    }

    /// The zoom slider's place: 0 the whole square, 1 all the way in.
    pub fn level(&self) -> f32 {
        (self.zoom - 1.) / (CROP_MOST - 1.)
    }

    /// Zoomed to the slider's place `level`.
    #[must_use]
    pub fn at_level(self, level: f32) -> Self {
        Self { zoom: 1. + (CROP_MOST - 1.) * level.clamp(0., 1.), ..self }.clamped()
    }

    /// A drag of `pan` and a pinch by `by` around `centre`, all in a frame `frame` across (its own pixels): the spot
    /// under the fingers stays under them.
    #[must_use]
    pub fn moved(self, frame: f32, centre: (f32, f32), pan: (f32, f32), by: f32) -> Self {
        let frame = frame.max(1.);
        let off = (centre.0 - frame / 2., centre.1 - frame / 2.);
        let before = frame * self.zoom / self.short();
        let under = (self.x + off.0 / before, self.y + off.1 / before);
        let zoom = (self.zoom * by).clamp(1., CROP_MOST);
        let after = frame * zoom / self.short();
        Self { x: under.0 - off.0 / after - pan.0 / after, y: under.1 - off.1 / after - pan.1 / after, zoom, ..self }
            .clamped()
    }

    /// The square shown, in pixels of the picture read `scale` times as large as this crop's.
    pub fn square(&self, scale: f32) -> Square {
        let (w, h) = ((self.width * scale).round().max(1.), (self.height * scale).round().max(1.));
        let side = self.side() * scale;
        let x = ((self.x * scale - side / 2.).max(0.)).round().min(w - 1.);
        let y = ((self.y * scale - side / 2.).max(0.)).round().min(h - 1.);
        let side = side.round().min(w - x).min(h - y).max(1.);
        Square { x: x as u32, y: y as u32, side: side as u32 }
    }
}

/// A picture ready to send: its bytes, its type, and its square when it's sent whole (a GIF).
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub crop: Option<String>,
}

/// The edit card's state.
#[derive(Clone, Debug, PartialEq)]
pub struct EditView {
    pub title: String,
    pub button: String,
    pub busy: bool,
    pub note: Option<String>,
    /// Delete shows (managing, never the main profile), and whether it's asking to be pressed again.
    pub delete: Option<bool>,
    /// Remove picture is asking to be pressed again.
    pub removing: bool,
    /// The saved picture is to go.
    pub removed: bool,
    /// Goes up each time the card is done with (saved or deleted): it closes.
    pub closed: u32,
    pub words: EditWords,
}

/// The edit card's fixed words; the asks say "Click" on a computer, "Press" elsewhere.
#[derive(Clone, Debug, PartialEq)]
pub struct EditWords {
    pub name: String,
    pub choose_another: String,
    pub remove_picture: String,
    pub remove_ask: String,
    pub delete: String,
    pub delete_ask: String,
    pub cancel: String,
}

impl Default for EditView {
    fn default() -> Self {
        Editor::default().view()
    }
}

/// What happened on the edit card.
#[derive(Clone, Debug, PartialEq)]
pub enum EditEvent {
    /// Opened for `profile` (None: a new one), by the account it's on.
    Opened { profile: Option<Profile>, account: Account },
    /// A picture picked: any removal asked for is off.
    Picked,
    /// A picture that couldn't be read for the cropper.
    PickFailed { words: String },
    /// Remove picture pressed (once to ask, again to do it).
    Remove,
    Save { name: String, picture: Option<Picture> },
    /// Delete pressed (once to ask, again to do it).
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Name,
    Picture,
    Delete,
}

/// The edit card. See the module's docs.
#[derive(Debug, Default)]
pub struct Editor {
    profile: Option<Profile>,
    /// On a computer (its asks say "Click").
    desktop: bool,
    /// A new profile made, its picture not in yet: Save again finishes it, never making a second one.
    made: Option<String>,
    deletable: bool,
    asking_delete: bool,
    removing: bool,
    removed: bool,
    busy: bool,
    note: Option<String>,
    closed: u32,
    /// The picture to send once the name's in.
    picture: Option<Picture>,
    asks: Asks<Step>,
}

impl Editor {
    fn post(&mut self, step: Step, path: &str, body: Value, cx: &Context) -> Effect {
        Effect::Request { id: self.asks.ask(step), request: api::post(&cx.base, &cx.who(), path, body.to_string()) }
    }

    fn id(&self) -> Option<String> {
        self.made.clone().or_else(|| self.profile.as_ref().map(|p| p.id.clone()))
    }

    fn path(&self, what: &str) -> Option<String> {
        self.id().map(|id| format!("{PROFILES}/{}/{what}", crate::sections::encode(&id)))
    }

    /// The name's in: the picture next, or done.
    fn picture_next(&mut self, cx: &Context) -> Vec<Effect> {
        let Some(path) = self.path("avatar") else { return self.done() };
        let (body, query) = match self.picture.take() {
            Some(p) => (Body::Bytes { bytes: p.bytes, content_type: p.content_type }, p.crop.unwrap_or_default()),
            None if self.removed => (Body::Bytes { bytes: vec![], content_type: "application/octet-stream".into() }, String::new()),
            None => return self.done(),
        };
        let mut request = api::post(&cx.base, &cx.who(), &format!("{path}{query}"), String::new());
        request.body = Some(body);
        vec![Effect::Request { id: self.asks.ask(Step::Picture), request }]
    }

    /// Saved or deleted: the card closes, and every place showing the profiles reads them again.
    fn done(&mut self) -> Vec<Effect> {
        self.busy = false;
        self.closed += 1;
        vec![Effect::Tell { event: AppEvent::ProfilesChanged }]
    }
}

impl Screen for Editor {
    type Event = EditEvent;
    type View = EditView;

    fn update(&mut self, event: EditEvent, cx: &Context) -> Vec<Effect> {
        match event {
            EditEvent::Opened { profile, account } => {
                let main = account.me().is_some_and(|m| m.index == 0);
                let deletable = main && profile.as_ref().is_some_and(|p| p.index != 0);
                let desktop = cx.platform.device == Device::Desktop;
                *self = Self { profile, deletable, desktop, closed: self.closed, ..Self::default() };
                vec![]
            }
            EditEvent::Picked => {
                self.removing = false;
                self.removed = false;
                self.note = None;
                vec![]
            }
            EditEvent::PickFailed { words } => {
                self.note = Some(words);
                vec![]
            }
            EditEvent::Remove => {
                if self.removing {
                    self.removing = false;
                    self.removed = self.profile.as_ref().is_some_and(|p| p.avatar.is_some());
                } else {
                    self.removing = true;
                }
                vec![]
            }
            EditEvent::Save { name, picture } => {
                let name: String = name.trim().chars().take(NAME_MOST).collect();
                if self.busy || name.is_empty() {
                    return vec![];
                }
                self.busy = true;
                self.note = None;
                self.picture = picture;
                let body = json!({"name": name});
                match self.path("update") {
                    Some(path) => vec![self.post(Step::Name, &path, body, cx)],
                    None => vec![self.post(Step::Name, PROFILES, body, cx)],
                }
            }
            EditEvent::Delete => {
                if !self.deletable || self.busy {
                    return vec![];
                }
                if !self.asking_delete {
                    self.asking_delete = true;
                    return vec![];
                }
                self.asking_delete = false;
                self.busy = true;
                self.note = None;
                match self.path("delete") {
                    Some(path) => vec![self.post(Step::Delete, &path, json!({}), cx)],
                    None => vec![],
                }
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        let Some(step) = self.asks.take(id) else { return vec![] };
        match (step, api::outcome(reply, cx.online)) {
            (Step::Name, Outcome::Body { body, .. }) => {
                if self.id().is_none() {
                    self.made = serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|o| o.get("id")?.as_str().map(String::from));
                }
                self.picture_next(cx)
            }
            (Step::Picture, Outcome::Body { .. }) => self.done(),
            (Step::Delete, Outcome::Body { .. }) => {
                let gone = self.profile.as_ref().map(|p| p.id.clone()).unwrap_or_default();
                let mut effects = self.done();
                effects.push(Effect::Tell { event: AppEvent::ProfileGone { id: gone } });
                effects
            }
            (_, Outcome::Failed { failure }) => {
                self.busy = false;
                self.note = Some(failure.words);
                vec![]
            }
            (_, Outcome::Unchanged) => vec![],
        }
    }

    fn view(&self) -> EditView {
        let press = if self.desktop { "Click" } else { "Press" };
        EditView {
            title: if self.profile.is_some() { "Edit profile" } else { "Add profile" }.into(),
            button: if self.busy { "Saving…" } else { "Save" }.into(),
            busy: self.busy,
            note: self.note.clone(),
            delete: self.deletable.then_some(self.asking_delete),
            removing: self.removing,
            removed: self.removed,
            closed: self.closed,
            words: EditWords {
                name: "Name".into(),
                choose_another: "Choose another".into(),
                remove_picture: "Remove picture".into(),
                remove_ask: format!("{press} again to remove"),
                delete: "Delete".into(),
                delete_ask: format!("{press} again to delete"),
                cancel: "Cancel".into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{response, test_context};

    fn account(me: usize, max: u32) -> Account {
        let profiles = (0..3)
            .map(|i| Profile { id: format!("p{i}"), name: format!("P{i}"), avatar: None, index: i, settings: None })
            .collect();
        Account { profiles, current: Some(format!("p{me}")), plus: false, max_profiles: max }
    }

    fn request(effects: &[Effect]) -> (u32, crate::screen::Request) {
        match effects {
            [Effect::Request { id, request }] => (*id, request.clone()),
            other => panic!("expected one request, got {other:?}"),
        }
    }

    #[test]
    fn the_main_profile_manages_them_all() {
        let t = tile(&account(0, 5));
        assert_eq!((t.title.as_str(), t.cards.len(), t.add.as_deref()), ("Profiles", 3, Some("Up to 5")));
        assert_eq!(t.cards.iter().map(|c| c.role.as_str()).collect::<Vec<_>>(), ["You", "Profile", "Profile"]);
        assert_eq!(t.cards.iter().map(|c| c.deletable).collect::<Vec<_>>(), [false, true, true]);
        assert_eq!(tile(&account(0, 3)).add, None);
        assert_eq!(tile(&account(0, u32::MAX)).add.as_deref(), Some(""));
        let other = tile(&account(1, 5));
        assert_eq!((other.title.as_str(), other.cards.len(), other.add), ("Your profile", 1, None));
        assert_eq!(other.cards[0].role, "You");
    }

    #[test]
    fn a_new_profile_is_made_then_given_its_picture() {
        let cx = test_context(0.);
        let mut e = Editor::default();
        e.update(EditEvent::Opened { profile: None, account: account(0, 5) }, &cx);
        assert_eq!(e.view().title, "Add profile");
        let picture = Picture { bytes: vec![1, 2], content_type: "image/gif".into(), crop: Some(crop_query(1, 2, 3)) };
        let (id, r) = request(&e.update(EditEvent::Save { name: "  Sam  ".into(), picture: Some(picture) }, &cx));
        assert!(r.url.ends_with("/api/profiles"));
        assert_eq!(e.view().button, "Saving…");
        let (id, r) = request(&e.answer(id, response(200, r#"{"id": "new"}"#, None), &cx));
        assert!(r.url.ends_with("/api/profiles/new/avatar?crop=1,2,3"));
        assert_eq!(r.body, Some(Body::Bytes { bytes: vec![1, 2], content_type: "image/gif".into() }));
        assert_eq!(e.answer(id, response(200, "{}", None), &cx), vec![Effect::Tell { event: AppEvent::ProfilesChanged }]);
        assert_eq!((e.view().closed, e.view().busy), (1, false));
    }

    #[test]
    fn the_asks_say_click_on_a_computer() {
        let mut cx = test_context(0.);
        let mut e = Editor::default();
        let p = account(0, 5).profiles[1].clone();
        e.update(EditEvent::Opened { profile: Some(p.clone()), account: account(0, 5) }, &cx);
        assert_eq!(e.view().words.delete_ask, "Press again to delete");
        cx.platform.device = crate::screen::Device::Desktop;
        e.update(EditEvent::Opened { profile: Some(p), account: account(0, 5) }, &cx);
        assert_eq!((e.view().words.remove_ask.as_str(), e.view().words.delete_ask.as_str()), ("Click again to remove", "Click again to delete"));
    }

    #[test]
    fn a_failed_picture_saves_again_without_a_second_profile() {
        let cx = test_context(0.);
        let mut e = Editor::default();
        e.update(EditEvent::Opened { profile: None, account: account(0, 5) }, &cx);
        let picture = Picture { bytes: vec![1], content_type: "image/jpeg".into(), crop: None };
        let (id, _) = request(&e.update(EditEvent::Save { name: "Sam".into(), picture: Some(picture.clone()) }, &cx));
        let (id, _) = request(&e.answer(id, response(200, r#"{"id": "new"}"#, None), &cx));
        e.answer(id, response(500, r#"{"error":"Try again."}"#, None), &cx);
        assert_eq!(e.view().note.as_deref(), Some("Try again."));
        let (_, r) = request(&e.update(EditEvent::Save { name: "Sam".into(), picture: Some(picture) }, &cx));
        assert!(r.url.ends_with("/api/profiles/new/update"));
    }

    #[test]
    fn removing_and_deleting_ask_twice() {
        let cx = test_context(0.);
        let mut e = Editor::default();
        let mut pictured = account(0, 5).profiles[1].clone();
        pictured.avatar = Some("/a.png".into());
        e.update(EditEvent::Opened { profile: Some(pictured), account: account(0, 5) }, &cx);
        e.update(EditEvent::Remove, &cx);
        assert!(e.view().removing && !e.view().removed);
        e.update(EditEvent::Remove, &cx);
        assert!(!e.view().removing && e.view().removed);
        let (id, _) = request(&e.update(EditEvent::Save { name: "P1".into(), picture: None }, &cx));
        let (id, r) = request(&e.answer(id, response(200, "{}", None), &cx));
        assert!(r.url.ends_with("/api/profiles/p1/avatar"));
        e.answer(id, response(200, "{}", None), &cx);
        assert_eq!(e.view().delete, Some(false));
        assert!(e.update(EditEvent::Delete, &cx).is_empty());
        assert_eq!(e.view().delete, Some(true));
        let (id, r) = request(&e.update(EditEvent::Delete, &cx));
        assert!(r.url.ends_with("/api/profiles/p1/delete"));
        let done = e.answer(id, response(200, "{}", None), &cx);
        assert!(done.contains(&Effect::Tell { event: AppEvent::ProfileGone { id: "p1".into() } }));
    }

    #[test]
    fn the_cropper_stays_inside_the_picture() {
        let c = Crop::new(400., 200.);
        assert_eq!((c.x, c.y, c.side()), (200., 100., 200.));
        assert_eq!(c.square(1.), Square { x: 100, y: 0, side: 200 });
        let far = c.moved(100., (50., 50.), (-1000., 0.), 1.);
        assert_eq!((far.x, far.y), (300., 100.));
        let zoomed = c.at_level(1.);
        assert_eq!((zoomed.zoom, zoomed.side(), zoomed.level()), (CROP_MOST, 50., 1.));
        assert_eq!(c.moved(100., (50., 50.), (0., 0.), 10.).zoom, CROP_MOST);
        // (a picture read at half its size: the square in its own pixels)
        assert_eq!(c.square(2.), Square { x: 200, y: 0, side: 400 });
    }

    #[test]
    fn the_main_profile_is_never_deleted() {
        let cx = test_context(0.);
        let mut e = Editor::default();
        e.update(EditEvent::Opened { profile: Some(account(0, 5).profiles[0].clone()), account: account(0, 5) }, &cx);
        assert_eq!(e.view().delete, None);
        assert!(e.update(EditEvent::Delete, &cx).is_empty());
    }
}
