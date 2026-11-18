//! A small retained UI on the OS compositor. A page is described as a tree of `El`s whenever something changes; the kit
//! lays it out with taffy and touches only the visuals that differ. Between changes the app does nothing at all: scrolling,
//! hover, fades and the caret's blink are animations the compositor plays by itself, as keyframes worked out here.
use crate::gfx::*;
use crate::platform::{self, Animation, Brush, Canvas, Clip, ColorBrush, Compositor, Cursor, Draw, Easing, Error, Fit, Layer, Pair, Paint, Rect, Result, Scalar, Surface, Visual, Window};
use crate::App;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::Instant;
use taffy::prelude::*;
use taffy::Overflow;

/// The kit's own settling curve: quick, then easing into place (hover growth, wheel scrolling, pictures fading in).
const SETTLE: Easing = Easing::Bezier([0.2, 0.8, 0.2, 1.0]);

pub type Cb = Rc<dyn Fn(&mut App)>;
/// Run as a drag moves: the fraction along the thing dragged, and whether it was let go.
pub type DragCb = Rc<dyn Fn(&mut App, f32, bool)>;

/// An element's key from its name (FNV-1a: thousands a frame, kept only in memory).
pub fn k(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3))
}
fn mix(a: u64, b: u64) -> u64 {
    (a ^ b.wrapping_mul(0x9E3779B97F4A7C15)).rotate_left(23).wrapping_mul(0xBF58476D1CE4E5B9)
}
/// CSS cubic-bezier(x1, y1, x2, y2) at time x: its y.
pub fn bezier(b: [f32; 4], x: f32) -> f32 {
    let [x1, y1, x2, y2] = b;
    let at = |a: f32, c: f32, t: f32| 3. * (1. - t) * (1. - t) * t * a + 3. * (1. - t) * t * t * c + t * t * t;
    let (mut lo, mut hi) = (0f32, 1f32);
    let mut t = x;
    for _ in 0..30 {
        let got = at(x1, x2, t);
        if (got - x).abs() < 1e-5 { break; }
        if got < x { lo = t } else { hi = t }
        t = (lo + hi) / 2.;
    }
    at(y1, y2, t)
}

#[derive(Clone, PartialEq, Debug, Default)]
pub enum Fill {
    #[default]
    None,
    Solid(u32),
    /// From (x0, y0) to (x1, y1), as fractions of the box.
    Linear(f32, f32, f32, f32, Vec<(f32, u32)>),
    /// Centre (cx, cy) as fractions of the box, radius as a fraction of its width.
    Radial(f32, f32, f32, Vec<(f32, u32)>),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Img {
    pub url: String,
    pub fallback: Option<String>,
    /// `filter: grayscale() brightness()`, baked in as it's decoded.
    pub gray: f32,
    pub bright: f32,
    /// Lifted toward white by this much (0 none, 1 white), its shape kept: a black logo made readable on the dark.
    pub lift: f32,
    pub crop: Option<(u32, u32, u32, u32)>,
    pub contain: bool,
    pub crop_y: f32,
    /// Fitted (contain), standing on its foot in the middle across, whatever `crop_y` says.
    pub centred: bool,
    pub fade_ms: i64,
    pub animated: bool,
    /// Darkened toward its left and its foot, baked in as it's decoded (the stage's words sit there): one layer, so
    /// its rounded corners have one edge, not the seam of shades clipped over it separately.
    pub shade: u8,
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Look {
    pub fill: Fill,
    pub radius: f32,
    pub border: f32,
    pub border_fill: Fill,
    pub text: Option<(String, TextStyle)>,
    /// The text drawn again around itself in this colour first: an outline (the Top 10 numbers).
    pub outline: Option<(f32, u32)>,
    pub icon: Option<(&'static str, f32, u32)>,
    /// An arc: (stroke, colour, how much of the way round). The spinner is .75, turned by the compositor; a ring's
    /// progress starts at the top.
    pub arc: Option<(f32, u32, f32)>,
    /// A line along the top edge (glass panels' lit top).
    pub top_line: Option<u32>,
    /// A Top 10 place's figure (1 to 10), drawn from rank_digits.
    pub rank: Option<usize>,
}
impl Look {
    fn plain(&self) -> bool {
        self.fill == Fill::None && self.border <= 0.
            && self.text.is_none() && self.icon.is_none() && self.arc.is_none() && self.top_line.is_none() && self.rank.is_none()
    }
    /// A flat colour with square corners needs no surface: the compositor fills it.
    fn flat(&self) -> Option<u32> {
        match self.fill {
            Fill::Solid(c) if self.radius == 0. && self.border == 0. && self.text.is_none() && self.icon.is_none() && self.arc.is_none() && self.top_line.is_none() && self.rank.is_none() => Some(c),
            _ => None,
        }
    }
}

/// The splash waits this long before it plays: the window opening and maximising first (they hid its start).
pub const SPLASH_DELAY: f32 = 350.;

/// How an element arrives (played once, when it's first built) — or, for the `Out` kinds, how it leaves: a page keeps
/// the leaving copy for a moment and builds it with one of these.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Anim {
    #[default]
    None,
    /// Fades in over this many ms.
    Fade(i64),
    /// Fades in rising this far, over this many ms (ease).
    Rise(f32, i64),
    /// A page opening: alpha 0 -> 1, scale .97 -> 1, 240 ms `out`.
    Page,
    /// A page closing: the reverse.
    PageOut,
    /// A menu growing out of a point (fractions of its box) on the menu spring: alpha min(1, 2.2x), scaleX .35 -> 1, scaleY .2 -> 1, y -(1-x)·10.
    Grow(f32, f32),
    /// The menu going back into its point, 130 ms accelerate.
    Shrink(f32, f32),
    /// The player's choices panel: alpha, y -6, scale .97 -> 1 from the top right, 240 ms.
    Drop,
    /// The same, reversed: the panel closing.
    DropOut,
    /// Play or pause just pressed, in the middle: in at once, a touch larger as it fades away (YouTube's, Netflix's).
    Flash,
    /// Moved in from this far to the right, over ms (FastOutSlowIn), keeping to its left edge.
    Shift(f32, i64),
    /// Fades in moving from (dx, dy) to its place, over ms (ease).
    Enter(f32, f32, i64),
    /// Fades out over ms (a leaving copy: the player's controls hiding).
    FadeOut(i64),
    /// Turns once round (1: clockwise, -1: the other way) over 420 ms ease: the player's jump.
    Spin(f32),
    /// Turned over (half round) near the end of every `ms`, resting the rest: an hourglass still waiting.
    Turn(i64),
    /// Fills across from the left, from this share of its width to all of it, linearly over ms (a countdown).
    FillX(f32, i64),
    /// A piece of the loading screen's name, laid where it ends up, played on the compositor's clock as the website's
    /// (zsplash.js): 0 "z" (slides in, then along by `a` as the name opens), 1 "un" (follows the z, shown from its
    /// left as it opens), 2 "f" (drops in on a spring, then along by `a`), 3 "lix" (follows the f, shown likewise).
    Splash(u8, f32),
    /// The splash's name opening, played backwards over ms: slid back by `dx` to where it's laid; folding (`true`),
    /// it closes up from its right as it fades, as "un" and "lix" opened out.
    Close(f32, i64, bool),
    /// One slice (the first number) of the band of light over the name (of the second), sweeping across again and
    /// again once the name's open.
    Shine(f32, f32),
    /// Grows out of a card it was over: from this scale to its own, fading in, over 220 ms (ease).
    /// Grows out of something smaller (from: its scale) around a point `y` down its middle.
    Pop(f32, f32),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Scroll {
    No,
    Y,
    X,
}

pub struct FieldSpec {
    pub style: TextStyle,
    pub secret: bool,
    pub offset: f32,
    pub change: Option<Cb>,
    pub submit: Option<Cb>,
}

/// One element: its layout, what it looks like, what it does.
pub struct El {
    pub style: Style,
    pub look: Look,
    pub kids: Vec<El>,
    pub key: u64,
    pub click: Option<Cb>,
    /// What a right-click on it does (a card's title page).
    pub context: Option<Cb>,
    pub hot: bool,
    pub grow_on_hover: f32,
    /// Opacity changes ease over this long (in and out alike) instead of jumping; 0: they jump.
    pub ease: i64,
    pub sink: f32,
    pub blur: Option<f32>,
    pub clip: bool,
    pub scroll: Scroll,
    pub image: Option<Img>,
    pub opacity: f32,
    pub blink: bool,
    pub shimmer: bool,
    pub spin: bool,
    pub anim: Anim,
    /// Moves to a new place by sliding there over this many ms, not jumping.
    pub slide: i64,
    /// The slide played as a way out: the way in's curve backwards (easing off from rest, quickening away).
    pub slide_back: bool,
    /// Turned by this many degrees, turning on the chevron spring when it changes.
    pub turn: f32,
    pub video: bool,
    pub drift: Option<(f32, f32)>,
    /// What's drawn (white) used as a stencil, filled with this colour; a new colour glides in on the compositor
    /// (a colour brush animated under the stencil: no second copy fading over the first).
    pub tint: Option<u32>,
    pub field: Option<FieldSpec>,
    pub drag: Option<DragCb>,
    /// Dragged up and down (its fraction 0 at the foot, 1 at the top), not across.
    pub drag_up: bool,
    pub hand: bool,
    /// The move cursor over it (and while it's dragged), not the hand: a thing moved about, as a picture in a cropper.
    pub grab: bool,
    /// Scaled by k from its top left corner, then moved by (dx, dy), by the compositor: its picture's drawn once, at
    /// its own size, however it's zoomed or moved after.
    pub transform: Option<(f32, f32, f32)>,
    pub decorative: bool,
    /// Kept exactly as it is on screen, not built (see `retain`).
    pub retain: bool,
    /// Its opacity follows the pointer over another element (see `fade_on_hover`).
    pub hover_fade: Option<(u64, f32, f32)>,
}

pub fn el() -> El {
    El {
        style: Style::default(), look: Look::default(), kids: Vec::new(), key: 0, click: None, context: None, hot: false, grow_on_hover: 1., ease: 0, sink: 1., blur: None, clip: false,
        scroll: Scroll::No, image: None, opacity: 1., blink: false, shimmer: false, spin: false, anim: Anim::None, slide: 0, slide_back: false, turn: 0., video: false, drift: None, tint: None, field: None, drag: None, drag_up: false, hand: false, grab: false, transform: None, decorative: false, retain: false, hover_fade: None,
    }
}
pub fn col() -> El {
    let mut e = el();
    e.style.flex_direction = FlexDirection::Column;
    e
}
pub fn row() -> El {
    let mut e = el();
    e.style.flex_direction = FlexDirection::Row;
    e.style.align_items = Some(AlignItems::Center);
    e
}
pub fn text(s: impl Into<String>, t: TextStyle) -> El {
    let mut e = el();
    e.look.text = Some((s.into(), t));
    e
}
pub fn icon(name: &'static str, size: f32, color: u32) -> El {
    let mut e = el().size(size, size);
    e.look.icon = Some((name, size, color));
    e
}

fn lp(v: f32) -> LengthPercentage {
    LengthPercentage::length(v)
}
fn lpa(v: f32) -> LengthPercentageAuto {
    LengthPercentageAuto::length(v)
}

impl El {
    pub fn key(mut self, s: &str) -> El {
        self.key = k(s);
        self
    }
    pub fn id(mut self, id: u64) -> El {
        self.key = id;
        self
    }
    pub fn kids(mut self, kids: Vec<El>) -> El {
        self.kids = kids;
        self
    }
    pub fn kid(mut self, kid: El) -> El {
        self.kids.push(kid);
        self
    }
    pub fn maybe(mut self, kid: Option<El>) -> El {
        if let Some(kid) = kid {
            self.kids.push(kid);
        }
        self
    }
    pub fn w(mut self, v: f32) -> El {
        self.style.size.width = Dimension::length(v);
        self
    }
    pub fn h(mut self, v: f32) -> El {
        self.style.size.height = Dimension::length(v);
        self
    }
    pub fn size(self, w: f32, h: f32) -> El {
        self.w(w).h(h)
    }
    pub fn fill_w(mut self) -> El {
        self.style.size.width = Dimension::percent(1.);
        self
    }
    pub fn fill_h(mut self) -> El {
        self.style.size.height = Dimension::percent(1.);
        self
    }
    pub fn fill(self) -> El {
        self.fill_w().fill_h()
    }
    pub fn pct_w(mut self, v: f32) -> El {
        self.style.size.width = Dimension::percent(v);
        self
    }
    pub fn pct_h(mut self, v: f32) -> El {
        self.style.size.height = Dimension::percent(v);
        self
    }
    pub fn min_h(mut self, v: f32) -> El {
        self.style.min_size.height = Dimension::length(v);
        self
    }
    pub fn max_h(mut self, v: f32) -> El {
        self.style.max_size.height = Dimension::length(v);
        self
    }
    pub fn max_w(mut self, v: f32) -> El {
        self.style.max_size.width = Dimension::length(v);
        self
    }
    pub fn grow(mut self) -> El {
        self.style.flex_grow = 1.;
        self.style.flex_basis = Dimension::length(0.);
        self.style.min_size.width = Dimension::length(0.);
        self
    }
    pub fn no_shrink(mut self) -> El {
        self.style.flex_shrink = 0.;
        self
    }
    pub fn pad(self, v: f32) -> El {
        self.pad4(v, v, v, v)
    }
    pub fn pad_xy(self, x: f32, y: f32) -> El {
        self.pad4(x, y, x, y)
    }
    pub fn pad4(mut self, l: f32, t: f32, r: f32, b: f32) -> El {
        self.style.padding = taffy::Rect { left: lp(l), right: lp(r), top: lp(t), bottom: lp(b) };
        self
    }
    pub fn margin(mut self, l: f32, t: f32, r: f32, b: f32) -> El {
        self.style.margin = taffy::Rect { left: lpa(l), right: lpa(r), top: lpa(t), bottom: lpa(b) };
        self
    }
    pub fn gap(mut self, v: f32) -> El {
        self.style.gap = taffy::Size { width: lp(v), height: lp(v) };
        self
    }
    pub fn wrap(mut self) -> El {
        self.style.flex_wrap = FlexWrap::Wrap;
        self
    }
    pub fn center(mut self) -> El {
        self.style.align_items = Some(AlignItems::Center);
        self.style.justify_content = Some(JustifyContent::Center);
        self
    }
    pub fn items_center(mut self) -> El {
        self.style.align_items = Some(AlignItems::Center);
        self
    }
    pub fn items_start(mut self) -> El {
        self.style.align_items = Some(AlignItems::FlexStart);
        self
    }
    /// Its children as tall (in a row) or as wide (in a column) as it is.
    pub fn items_stretch(mut self) -> El {
        self.style.align_items = Some(AlignItems::Stretch);
        self
    }
    pub fn items_end(mut self) -> El {
        self.style.align_items = Some(AlignItems::FlexEnd);
        self
    }
    pub fn justify_end(mut self) -> El {
        self.style.justify_content = Some(JustifyContent::FlexEnd);
        self
    }
    /// Placed by hand inside its parent, out of the flow. NaN leaves an edge free.
    pub fn at(mut self, left: f32, top: f32, right: f32, bottom: f32) -> El {
        self.style.position = Position::Absolute;
        let e = |v: f32| if v.is_nan() { LengthPercentageAuto::auto() } else { lpa(v) };
        self.style.inset = taffy::Rect { left: e(left), right: e(right), top: e(top), bottom: e(bottom) };
        self
    }
    /// Over the whole of its parent.
    pub fn decorative(mut self) -> El {
        self.decorative = true;
        self
    }
    pub fn cover(self) -> El {
        self.at(0., 0., 0., 0.)
    }
    /// The node with this key kept exactly as it is on screen, everything in it too, without being built or matched
    /// again (a page hidden under another): its pictures stay in use. Only for a node already there, at the same size
    /// (`Kit::can_retain`).
    pub fn retain(mut self) -> El {
        self.retain = true;
        self
    }
    /// Its opacity `on` while the pointer's within the element with this key, `off` otherwise, easing between them
    /// over its `ease`: the kit plays the change itself as the pointer moves, with nothing built again (a card's
    /// details fading in under the pointer).
    pub fn fade_on_hover(mut self, key: u64, off: f32, on: f32) -> El {
        self.hover_fade = Some((key, off, on));
        self
    }
    pub fn bg(mut self, c: u32) -> El {
        self.look.fill = Fill::Solid(c);
        self
    }
    pub fn paint(mut self, f: Fill) -> El {
        self.look.fill = f;
        self
    }
    pub fn radius(mut self, r: f32) -> El {
        self.look.radius = r;
        self
    }
    pub fn border(mut self, w: f32, c: u32) -> El {
        self.look.border = w;
        self.look.border_fill = Fill::Solid(c);
        self
    }
    pub fn click(mut self, f: impl Fn(&mut App) + 'static) -> El {
        self.click = Some(Rc::new(f));
        self.hot = true;
        self.hand = true;
        self
    }
    /// A right-click on it does this (the nearest thing under the pointer that has one).
    pub fn right_click(mut self, f: impl Fn(&mut App) + 'static) -> El {
        self.context = Some(Rc::new(f));
        self.hot = true;
        self
    }
    /// Lit under the pointer: it has to have a key to be remembered by.
    pub fn hover(mut self) -> El {
        self.hot = true;
        self
    }
    /// Its opacity eases to a new value over `ms` (from wherever it is), whichever way it goes.
    pub fn ease(mut self, ms: i64) -> El {
        self.ease = ms;
        self
    }
    pub fn grow_hover(mut self, s: f32) -> El {
        self.grow_on_hover = s;
        self.hot = true;
        self
    }
    /// Sinks a touch as it's pressed.
    pub fn sink(mut self, s: f32) -> El {
        self.sink = s;
        self
    }
    /// What's behind, saturated (`GLASS_SATURATION`) and blurred by `sigma`: the sheet backdrop and the sign-in wall.
    pub fn blur(mut self, sigma: f32) -> El {
        self.blur = Some(sigma);
        self.clip = true;
        self
    }
    pub fn anim(mut self, a: Anim) -> El {
        self.anim = a;
        self
    }
    pub fn slide(mut self, ms: i64) -> El {
        self.slide = ms;
        self
    }
    /// A slide away: the slide in played backwards (see `slide_back`).
    pub fn slide_away(mut self, ms: i64) -> El {
        self.slide = ms;
        self.slide_back = true;
        self
    }
    pub fn turn(mut self, degrees: f32) -> El {
        self.turn = degrees;
        self
    }
    pub fn top_line(mut self, c: u32) -> El {
        self.look.top_line = Some(c);
        self
    }
    pub fn rank_figure(mut self, place: usize) -> El {
        self.look.rank = Some(place);
        self
    }
    pub fn outline(mut self, width: f32, c: u32) -> El {
        self.look.outline = Some((width, c));
        self
    }
    /// Drawn white as a stencil and filled with this colour, a new one gliding in (see `tint` on El).
    pub fn tint(mut self, c: u32) -> El {
        self.tint = Some(c);
        self
    }
    pub fn clip(mut self) -> El {
        self.clip = true;
        self
    }
    pub fn scroll_y(mut self) -> El {
        self.scroll = Scroll::Y;
        self.clip = true;
        self.style.overflow.y = Overflow::Scroll;
        self.style.scrollbar_width = 0.;
        self
    }
    pub fn scroll_x(mut self) -> El {
        self.scroll = Scroll::X;
        self.clip = true;
        self.style.overflow.x = Overflow::Scroll;
        self.style.scrollbar_width = 0.;
        self
    }
    pub fn image(mut self, url: impl Into<String>) -> El {
        self.image = Some(Img { url: url.into(), fallback: None, gray: 0., bright: 1., lift: 0., crop: None, contain: false, crop_y: 0.5, centred: false, fade_ms: 250, animated: false, shade: 0 });
        self
    }
    pub fn animate_gif(mut self) -> El {
        if let Some(image) = &mut self.image { image.animated = true; }
        self
    }
    /// The stage's shading baked into the picture (see Img::shade).
    pub fn shade(mut self) -> El {
        if let Some(image) = &mut self.image {
            image.shade = 1;
        }
        self
    }
    /// A card's shade baked in (2: a poster's foot, deep, for its name; 3: a landscape card's foot, for its logo).
    pub fn shade_as(mut self, kind: u8) -> El {
        if let Some(image) = &mut self.image {
            image.shade = kind;
        }
        self
    }
    pub fn backdrop(mut self, crop_y: f32, fade_ms: i64) -> El {
        if let Some(image) = &mut self.image {
            image.crop_y = crop_y;
            image.fade_ms = fade_ms;
        }
        self
    }
    pub fn contain(mut self) -> El {
        if let Some(i) = &mut self.image { i.contain = true; }
        self
    }
    /// Fitted, on its foot in the middle across (a logo centred on a poster).
    pub fn centred(mut self) -> El {
        if let Some(i) = &mut self.image { i.centred = true; }
        self
    }
    pub fn crop(mut self, x: u32, y: u32, w: u32, h: u32) -> El {
        if let Some(i) = &mut self.image { i.crop = Some((x, y, w, h)); }
        self
    }
    pub fn opacity(mut self, o: f32) -> El {
        self.opacity = o;
        self
    }
    pub fn shimmer(mut self) -> El {
        self.shimmer = true;
        self
    }
    /// Slides by (dy, seconds) forever: the login wall's columns.
    pub fn drift(mut self, dy: f32, secs: f32) -> El {
        self.drift = Some((dy, secs));
        self
    }
    pub fn video(mut self) -> El {
        self.video = true;
        self
    }
    /// Dragged up and down: 0 at its foot, 1 at its top.
    pub fn drag_up(mut self, f: impl Fn(&mut App, f32, bool) + 'static) -> El {
        self.drag_up = true;
        self.drag(f)
    }
    /// The move cursor (see `grab`).
    pub fn grab(mut self) -> El {
        self.grab = true;
        self
    }
    /// Scaled by k from its corner, then moved by (dx, dy), on the compositor (see `transform`).
    pub fn transform(mut self, dx: f32, dy: f32, k: f32) -> El {
        self.transform = Some((dx, dy, k));
        self
    }
    pub fn drag(mut self, f: impl Fn(&mut App, f32, bool) + 'static) -> El {
        self.drag = Some(Rc::new(f));
        self.hot = true;
        self.hand = true;
        self
    }
    /// A 270° arc, stroke 9% of the size, round caps, one turn per 900 ms.
    /// How far along, as an arc from the top (a download's ring).
    pub fn progress_arc(mut self, stroke: f32, color: u32, share: f32) -> El {
        self.look.arc = Some((stroke, color, share.clamp(0., 1.)));
        self
    }
    pub fn spinner(mut self, size: f32, color: u32) -> El {
        self.look.arc = Some((size * 0.09, color, 0.75));
        self.spin = true;
        self.size(size, size)
    }
}

// ---------- what's on screen ----------
/// What an element draws (text, an icon, a rounded fill...): its visual over the base, and where it's painted.
struct Skin {
    vis: Visual,
    brush: Brush,
    place: Place,
}

enum Place {
    /// A surface of its own (a big skin, or a spinning one).
    Own(Surface),
    /// A part of one of the shared surfaces (see `Atlas`).
    Shared(Region),
}

impl Skin {
    fn paint(&self, g: &Gfx, draw: impl FnOnce(&mut Canvas<'_>) -> Result<()>) -> Result<bool> {
        match &self.place {
            Place::Own(surface) => g.paint(surface, draw),
            Place::Shared(r) => g.os.paint_in(&r.surface, r.x, r.y, r.w, r.h, g.scale, draw),
        }
    }
}

// ---------- the skin atlas ----------
/// Small skins (most text, icons, badges, small buttons) share a few big surfaces instead of each having its own:
/// far fewer GPU allocations, each one far from the minimum size. Each skin has its own part, whole pixels, with a
/// transparent pixel between it and the next (and the edge), so sampling one never picks up another. Parts come in
/// shelves of one height; a part let go is used again, a shelf emptied is free for any height that fits it, and a
/// surface emptied goes.
const ATLAS: u32 = 1024;

struct Shelf {
    y: u32,
    h: u32,
    /// Where the next part goes along it; parts let go, (x, width) to use again.
    end: u32,
    free: Vec<(u32, u32)>,
    live: u32,
}

struct AtlasPage {
    id: u64,
    surface: Surface,
    shelves: Vec<Shelf>,
    live: u32,
}

#[derive(Default)]
struct Atlas {
    pages: Vec<AtlasPage>,
    next: u64,
}

/// One skin's part of a shared surface: given back when the skin goes.
struct Region {
    atlas: Rc<std::cell::RefCell<Atlas>>,
    page: u64,
    shelf: usize,
    surface: Surface,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl Drop for Region {
    fn drop(&mut self) {
        if let Ok(mut atlas) = self.atlas.try_borrow_mut() {
            atlas.free(self.page, self.shelf, self.x, self.w + 1);
        }
    }
}

impl Atlas {
    /// A part w by h pixels (a pixel's gap kept right and below it): (page, shelf, x, y), making a new surface if no
    /// page has room.
    fn take(&mut self, g: &Gfx, w: u32, h: u32) -> Result<(u64, usize, u32, u32, Surface)> {
        let (need_w, need_h) = (w + 1, (h + 1).div_ceil(4) * 4);
        for page in &mut self.pages {
            let mut found = None;
            for (i, shelf) in page.shelves.iter_mut().enumerate() {
                // (a shelf of about this height: no more than half again as tall, so little goes to waste)
                if shelf.h < need_h || shelf.h > need_h + need_h / 2 { continue; }
                if let Some(j) = shelf.free.iter().position(|(_, fw)| *fw >= need_w) {
                    let (fx, fw) = shelf.free.swap_remove(j);
                    if fw > need_w { shelf.free.push((fx + need_w, fw - need_w)); }
                    found = Some((i, fx, shelf.y));
                    break;
                }
                if shelf.end + need_w <= ATLAS {
                    found = Some((i, shelf.end, shelf.y));
                    shelf.end += need_w;
                    break;
                }
            }
            if found.is_none() {
                let top = page.shelves.last().map_or(1, |s| s.y + s.h);
                if top + need_h <= ATLAS && need_w < ATLAS {
                    page.shelves.push(Shelf { y: top, h: need_h, end: 1 + need_w, free: vec![], live: 0 });
                    found = Some((page.shelves.len() - 1, 1, top));
                }
            }
            if let Some((i, x, y)) = found {
                page.shelves[i].live += 1;
                page.live += 1;
                return Ok((page.id, i, x, y, page.surface.clone()));
            }
        }
        // (a new shared surface, cleared all over once: its gaps stay clear for good)
        let surface = g.os.surface(ATLAS, ATLAS)?;
        g.paint(&surface, |_| Ok(()))?;
        self.next += 1;
        let id = self.next;
        self.pages.push(AtlasPage { id, surface: surface.clone(), shelves: vec![Shelf { y: 1, h: need_h, end: 1 + need_w, free: vec![], live: 1 }], live: 1 });
        Ok((id, 0, 1, 1, surface))
    }

    fn free(&mut self, page: u64, shelf: usize, x: u32, w: u32) {
        let Some(p) = self.pages.iter_mut().find(|p| p.id == page) else { return };
        let Some(s) = p.shelves.get_mut(shelf) else { return };
        s.live = s.live.saturating_sub(1);
        p.live = p.live.saturating_sub(1);
        if s.live == 0 {
            // (emptied: the whole shelf free again, from its start)
            s.free.clear();
            s.end = 1;
        } else {
            s.free.push((x, w));
        }
    }

    /// Surfaces with nothing on them go (all of them with `all`: the device was replaced, their pixels are gone),
    /// but one is kept for the next skins.
    fn sweep(&mut self, all: bool) {
        if all {
            self.pages.clear();
            return;
        }
        let mut kept = self.pages.iter().any(|p| p.live > 0);
        self.pages.retain(|p| {
            if p.live > 0 { return true; }
            if kept { return false; }
            kept = true;
            true
        });
    }
}

struct Node {
    key: u64,
    vis: Visual,
    /// Scrolled content sits in here.
    inner: Option<Visual>,
    skin: Option<Skin>,
    look: Look,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    kids: Vec<Node>,
    click: Option<Cb>,
    context: Option<Cb>,
    drag: Option<DragCb>,
    drag_up: bool,
    hot: bool,
    hand: bool,
    grab: bool,
    transform: Option<(f32, f32, f32)>,
    decorative: bool,
    grow: f32,
    sink: f32,
    clip: bool,
    clipped: (f32, f32, f32),
    /// Which blur it has (0: none), so a changed one gets a new brush.
    glass: u64,
    scroll: Scroll,
    image: Option<(u64, bool)>,
    /// How its picture goes on once it's in: (fade ms, fitted inside, a glow's colour), as its El last said.
    image_how: (i64, bool, Option<u32>),
    opacity: f32,
    field: Option<FieldSpec>,
    video: bool,
    turn: f32,
    /// Its stencil's colour brush and the colour it's going to (an El with a tint).
    tint: Option<(ColorBrush, u32)>,
    /// The way out it last played (an existing node given a way out plays it, as the reverse of its way in).
    out: Anim,
    /// Taken out of the compositor's work (IsVisible off): at opacity 0 once its fade there has played.
    hidden: bool,
    hide_at: Option<Instant>,
    /// A skeleton's sweeping band: stopped and hidden once a picture over it is fully in.
    band: Option<Band>,
    /// A picture filling its box: when it's fully in (its fade played), so a band under it can stop.
    opaque_at: Option<Instant>,
    /// Its opacity following the pointer over an element (El::hover_fade), how long it eases, and whether, faded to
    /// nothing, it can leave the compositor's work.
    hover_fade: Option<(u64, f32, f32)>,
    ease: i64,
    lone: bool,
}

struct Band {
    /// When it began sweeping, and the width its sweep was made for (a restart picks up in step, as if never stopped).
    since: Instant,
    width: f32,
    running: bool,
    stop_at: Option<Instant>,
}

#[derive(Default, Clone, Copy)]
pub struct ScrollState {
    pub pos: f32,
    pub max: f32,
    pub built_at: f32,
}

#[derive(Default, Clone)]
pub struct Edit {
    pub text: String,
    /// Caret and the other end of the selection, in UTF-16 units, as text layout counts them.
    pub caret: usize,
    pub anchor: usize,
}

struct Pic {
    brush: Brush,
    bytes: usize,
    used: u64,
    /// An animated picture's frames, all in its one surface, played by the compositor: whether they're playing.
    strip: Option<bool>,
}

pub struct Kit {
    pub g: Gfx,
    tree: Option<Node>,
    pub width: f32,
    pub height: f32,
    hover: u64,
    hover_path: Vec<u64>,
    /// The elements whose hover the pages asked about while being built (and whether they asked which one is under
    /// the pointer): building now, and as last built. The pointer moving over any other changes nothing they show
    /// but what the kit plays itself (growing, hover fades), so nothing is built again for it.
    asking: std::cell::RefCell<HashSet<u64>>,
    asked: HashSet<u64>,
    asking_hover: std::cell::Cell<bool>,
    asked_hover: bool,
    growing: u64,
    pub pressed: u64,
    pub focus: u64,
    pub edits: HashMap<u64, Edit>,
    pub scrolls: HashMap<u64, ScrollState>,
    pics: HashMap<u64, Pic>,
    loading: HashMap<u64, tokio::task::JoinHandle<()>>,
    wanted: std::collections::HashSet<u64>,
    animated_wanted: HashSet<u64>,
    /// The window's minimised: animated pictures held where they are.
    still: bool,
    failed_pictures: HashMap<u64, std::time::Instant>,
    pic_bytes: usize,
    uploads: std::collections::VecDeque<Upload>,
    frame: u64,
    pub video_brush: Option<Brush>,
    pub dirty: bool,
    pub pointer: (f32, f32),
    layout: Option<TaffyTree<Measured>>,
    /// The picture each node (by key) last showed: built again, it shows it without fading in again.
    seen: HashMap<u64, u64>,
    /// The next time a band or a hidden layer is due to settle (see `tick`).
    wake: Option<Instant>,
    /// Kept nodes (El::retain) that need building again: a picture came in for one that a frame has to put on.
    unkept: HashSet<u64>,
    /// The shared surfaces small skins are painted on.
    atlas: Rc<std::cell::RefCell<Atlas>>,
    now: Instant,
    /// What's being dragged: its handler, where it starts and how long it is (across, or up when the last is true).
    dragging: Option<(DragCb, f32, f32, bool)>,
    /// What's dragged is moved about (the move cursor stays while it is).
    grabbing: bool,
}

/// Where a node is built: its parent's origin in the window, and the areas (window coordinates, None for no limit)
/// outside which children are left out and pictures wait.
#[derive(Clone, Copy)]
struct Within {
    origin: (f32, f32),
    viewport: Option<[f32; 4]>,
    image_viewport: Option<[f32; 4]>,
}

struct Upload {
    key: u64,
    pixels: Vec<u8>,
    pw: u32,
    ph: u32,
    w: f32,
    h: f32,
    radius: f32,
    frames: Option<Vec<crate::images::AnimationFrame>>,
}

const PIC_MOST: usize = 96 * 1024 * 1024;

impl Kit {
    pub fn new(window: &Window, scale: f32, width: f32, height: f32, fonts: &std::path::Path) -> Result<Kit> {
        let g = Gfx::new(platform::Graphics::new(window, fonts)?, scale);
        Ok(Kit {
            g, tree: None, width, height, hover: 0, grabbing: false, hover_path: Vec::new(), asking: Default::default(), asked: HashSet::new(), asking_hover: Default::default(), asked_hover: false, growing: 0, pressed: 0, focus: 0, edits: HashMap::new(), scrolls: HashMap::new(), pics: HashMap::new(), loading: HashMap::new(), wanted: Default::default(), animated_wanted: HashSet::new(), still: false, failed_pictures: HashMap::new(),
            pic_bytes: 0, uploads: Default::default(), frame: 0, video_brush: None, dirty: true, pointer: (-1., -1.), layout: None, seen: HashMap::new(), wake: None, unkept: HashSet::new(), atlas: Default::default(), now: Instant::now(), dragging: None,
        })
    }

    pub fn text_of(&self, key: &str) -> String {
        self.edits.get(&k(key)).map(|e| e.text.clone()).unwrap_or_default()
    }
    pub fn set_text(&mut self, key: &str, s: &str) {
        let n = s.encode_utf16().count();
        self.edits.insert(k(key), Edit { text: s.to_string(), caret: n, anchor: n });
        self.dirty = true;
    }
    pub fn scroll_of(&self, key: u64) -> ScrollState {
        self.scrolls.get(&key).copied().unwrap_or_default()
    }
    pub fn scale_to_dpi(&mut self, scale: f32) {
        if self.g.scale == scale { return; }
        self.g.scale = scale;
        self.clear_visuals();
    }

    pub fn clear_visuals(&mut self) {
        self.tree = None;
        let _ = self.g.os.root().remove_all();
        self.pics.clear();
        for (_, job) in self.loading.drain() { job.abort(); }
        self.failed_pictures.clear();
        self.uploads.clear();
        self.animated_wanted.clear();
        self.unkept.clear();
        self.atlas.borrow_mut().sweep(true);
        self.pic_bytes = 0;
        self.dirty = true;
    }

    // ---------- layout and reconcile ----------
    pub fn commit(&mut self, mut root: El) -> Result<()> {
        // (what this tree's pages asked about the pointer as they were built)
        self.asked = std::mem::take(self.asking.get_mut());
        self.asked_hover = self.asking_hover.replace(false);
        self.frame += 1;
        self.now = Instant::now();
        self.flush_uploads();
        self.wanted.clear();
        self.animated_wanted.clear();
        // (one layout tree kept and cleared each frame: its storage reused, not made afresh)
        let mut t = self.layout.take().unwrap_or_else(TaffyTree::new);
        t.clear();
        t.disable_rounding();
        let id = add(&mut t, &mut root);
        let g = &self.g;
        t.compute_layout_with_measure(id, taffy::Size { width: AvailableSpace::Definite(self.width), height: AvailableSpace::Definite(self.height) }, |known, avail, _, ctx, _| {
            let Some(Some(text)) = ctx else { return taffy::Size::ZERO };
            // (the El's own text, alive and unmoved until the layout's done: not a copy of every string)
            let (s, ts) = unsafe { &**text };
            let w = known.width.unwrap_or(match avail.width {
                AvailableSpace::Definite(w) => w,
                AvailableSpace::MinContent => 0.,
                AvailableSpace::MaxContent => 1e5,
            });
            let (mw, mh) = g.measure(s, ts, w.max(1.));
            taffy::Size { width: known.width.unwrap_or(mw), height: known.height.unwrap_or(mh) }
        })
        .map_err(|e| Error::new(e.to_string()))?;
        root.key = 1;
        let old = self.tree.take();
        let node = self.reconcile(&t, id, root, old, 1, Within { origin: (0., 0.), viewport: None, image_viewport: None });
        self.layout = Some(t);
        let node = match node {
            Ok(node) => node,
            Err(error) => {
                self.g.os.root().remove_all()?;
                return Err(error);
            }
        };
        if !self.g.os.root().has_children()? {
            self.g.os.root().insert_top(&node.vis)?;
        }
        // (a scroll no longer drawn is forgotten: a page or a tab left and come back to starts at its top)
        let mut showing = HashSet::new();
        fn scrolled(n: &Node, into: &mut HashSet<u64>) {
            if n.scroll != Scroll::No { into.insert(n.key); }
            for kid in &n.kids { scrolled(kid, into); }
        }
        scrolled(&node, &mut showing);
        self.scrolls.retain(|key, _| showing.contains(key));
        self.tree = Some(node);
        self.loading.retain(|picture, job| {
            if self.wanted.contains(picture) { true } else { job.abort(); false }
        });
        // (pictures still waiting go on in the frames after, by apply_uploads)
        self.dirty = false;
        self.update_hover(false);
        self.trim_pics();
        self.play_strips();
        self.atlas.borrow_mut().sweep(false);
        self.tick();
        Ok(())
    }

    /// Bands whose picture is fully in stop, and layers faded to nothing leave the compositor's work, once due (called
    /// after each frame and from the window's slow timer).
    pub fn tick(&mut self) {
        let now = Instant::now();
        if self.wake.is_none_or(|at| at > now) { return; }
        fn walk(n: &mut Node, now: Instant, next: &mut Option<Instant>) {
            if let Some(at) = n.hide_at {
                if at <= now {
                    n.hide_at = None;
                    n.hidden = true;
                    let _ = n.vis.set_visible(false);
                } else {
                    *next = Some(next.map_or(at, |t| t.min(at)));
                }
            }
            if let Some(b) = n.band.as_mut() {
                if let Some(at) = b.stop_at {
                    if at <= now {
                        b.stop_at = None;
                        if b.running {
                            b.running = false;
                            let _ = n.vis.stop(Scalar::OffsetX);
                            let _ = n.vis.set_visible(false);
                        }
                    } else {
                        *next = Some(next.map_or(at, |t| t.min(at)));
                    }
                }
            }
            for kid in &mut n.kids { walk(kid, now, next); }
        }
        let mut next = None;
        if let Some(root) = self.tree.as_mut() { walk(root, now, &mut next); }
        self.wake = next;
    }
    fn wake_at(&mut self, at: Instant) {
        self.wake = Some(self.wake.map_or(at, |t| t.min(at)));
    }

    /// The bands among these children (a skeleton's) against the pictures over them: one stops once a picture filling
    /// the box above it has faded fully in; one stopped sweeps again (in step) when its picture goes.
    fn settle_bands(&mut self, kids: &mut [Node], w: f32, h: f32) -> Result<()> {
        if !kids.iter().any(|kid| kid.band.is_some()) { return Ok(()); }
        let now = self.now;
        for i in 0..kids.len() {
            if kids[i].band.is_none() { continue; }
            let covered = kids[i + 1..].iter()
                .filter(|o| o.x == 0. && o.y == 0. && o.w == w && o.h == h && o.opacity >= 1. && !o.hidden && o.hide_at.is_none())
                .filter_map(|o| o.opaque_at).min();
            let kid = &mut kids[i];
            let Some(b) = kid.band.as_mut() else { continue };
            match covered {
                Some(at) if at <= now => {
                    b.stop_at = None;
                    if b.running {
                        b.running = false;
                        kid.vis.stop(Scalar::OffsetX)?;
                        kid.vis.set_visible(false)?;
                    }
                }
                _ => {
                    b.stop_at = covered;
                    if !b.running {
                        b.running = true;
                        let (since, width) = (b.since, b.width);
                        kid.vis.set_visible(!kid.hidden)?;
                        self.shimmer(&kid.vis, width)?;
                        // (picked up where it would be had it never stopped)
                        let phase = (now.duration_since(since).as_millis() % 1600) as f32 / 1600.;
                        kid.vis.seek(Scalar::OffsetX, phase)?;
                    }
                    if let Some(at) = covered { self.wake_at(at); }
                }
            }
        }
        Ok(())
    }

    fn sprite(&self) -> Result<Visual> {
        self.g.os.sprite()
    }

    fn reconcile(&mut self, t: &TaffyTree<Measured>, id: NodeId, mut e: El, old: Option<Node>, key: u64, within: Within) -> Result<Node> {
        let Within { origin, viewport, image_viewport } = within;
        let l = t.layout(id).map_err(|e| Error::new(e.to_string()))?;
        let (x, y, w, h) = (l.location.x, l.location.y, l.size.width, l.size.height);
        if !self.unkept.is_empty() && !e.retain {
            self.unkept.remove(&key);
        }
        if e.retain {
            if let Some(mut n) = old {
                // (kept as it is: its pictures still in use; one that came in and needs a frame to go on, or a
                // different size, has it built again in the next frame)
                if self.keep(&mut n)? || n.x != x || n.y != y || n.w != w || n.h != h {
                    self.unkept.insert(key);
                    self.dirty = true;
                }
                return Ok(n);
            }
        }
        let (content_w, content_h) = (l.content_size.width, l.content_size.height);
        let s = self.g.scale;
        let (absolute_x, absolute_y) = (origin.0 + x, origin.1 + y);
        let mut child_origin = (absolute_x, absolute_y);
        let mut child_view = viewport;
        let mut child_images = image_viewport;
        let fresh = old.is_none();
        let mut n = match old {
            Some(n) => n,
            None => Node {
                key, vis: self.sprite()?, inner: None, skin: None, look: Look::default(), x: f32::NAN, y: 0., w: f32::NAN, h: 0., kids: Vec::new(), click: None, context: None, drag: None, drag_up: false, hot: false, hand: false, grab: false, transform: None, decorative: false, grow: 1., sink: 1., clip: false,
                clipped: (0., 0., 0.), glass: 0, scroll: Scroll::No, image: None, opacity: 1., field: None, video: false, turn: 0., out: Anim::None, tint: None,
                hidden: false, hide_at: None, band: None, opaque_at: None, image_how: (0, false, None), hover_fade: None, ease: 0, lone: false,
            },
        };
        if n.x != x || n.y != y {
            let to = [(x * s).round(), (y * s).round()];
            // (the place set first, then the slide started from where it was: setting a property after starting an
            // animation on it stops the animation, so it jumped instead of sliding)
            n.vis.set_offset(to[0], to[1])?;
            if e.slide > 0 && !fresh && n.x.is_finite() {
                // (a way out: EASE_EASE turned round in time, so it leaves as it came, backwards)
                let curve = if e.slide_back { [0.64, 0.0, 0.78, 0.0] } else { crate::tokens::EASE_EASE };
                let a = Animation::new(e.slide).key(0., [(n.x * s).round(), (n.y * s).round()], Easing::Linear).key(1., to, Easing::Bezier(curve));
                self.g.os.animate_pair(&n.vis, Pair::Offset, &a)?;
            }
            // (moving it stops a drift: the window resized, maximised or came back from the taskbar; start it again)
            if !fresh {
                if let Some((dy, secs)) = e.drift {
                    self.drift(&n.vis, dy, secs)?;
                }
            }
            n.x = x;
            n.y = y;
        }
        let resized = n.w != w || n.h != h;
        if resized {
            n.vis.set_size(w * s, h * s)?;
            n.vis.set_centre(w * s / 2., h * s / 2.)?;
            n.w = w;
            n.h = h;
        }
        if n.turn != e.turn {
            if fresh {
                n.vis.set_rotation(e.turn)?;
            } else {
                let (from, to) = (n.turn, e.turn);
                self.g.os.animate(&n.vis, Scalar::Rotation, &spring(crate::tokens::SPRING_CHEVRON, |v| from + (to - from) * v))?;
            }
            n.turn = e.turn;
        }
        let leaving = matches!(e.anim, Anim::PageOut | Anim::FadeOut(_));
        if let Some((over, off, on)) = e.hover_fade {
            e.opacity = if self.hover_path.contains(&over) { on } else { off };
        }
        n.hover_fade = e.hover_fade;
        n.ease = e.ease;
        n.lone = e.image.is_none() && !e.blink && !leaving;
        if n.opacity != e.opacity {
            // (nothing at all to show: out of the compositor's work once its fade there has played; never one whose
            // own opacity something else plays: a way in or out, a picture's fade, a caret)
            let alone = e.image.is_none() && !e.blink && if fresh { e.anim == Anim::None } else { !leaving };
            self.set_opacity(&mut n, e.opacity, e.ease, fresh, alone)?;
        }
        n.click = e.click;
        n.context = e.context;
        n.drag = e.drag;
        n.drag_up = e.drag_up;
        n.hot = e.hot;
        n.hand = e.hand;
        n.grab = e.grab;
        if n.transform != e.transform {
            let (dx, dy, k) = e.transform.unwrap_or((0., 0., 1.));
            n.vis.set_transform(dx * s, dy * s, k)?;
            n.transform = e.transform;
        }
        n.decorative = e.decorative;
        n.grow = e.grow_on_hover;
        n.sink = e.sink;
        n.scroll = e.scroll;
        n.field = e.field;

        // Clipping: a rounded blur gets a shape, plain overflow a rectangle.
        let want = if e.clip { (w, h, e.look.radius) } else { (0., 0., 0.) };
        if n.clip != e.clip || (e.clip && n.clipped != want) {
            let clip = if !e.clip {
                Clip::None
            } else if e.look.radius > 0. {
                Clip::Rounded { w: w * s, h: h * s, radius: e.look.radius.min(w / 2.).min(h / 2.) * s }
            } else {
                Clip::Inset
            };
            self.g.os.clip(&n.vis, clip)?;
            n.clip = e.clip;
            n.clipped = want;
        }

        // The base: a blur, a picture, video, or a flat colour.
        if n.glass != 0 && e.blur.is_none() {
            n.vis.set_brush(None)?;
            n.glass = 0;
            n.look = Look::default();
            n.image = None;
        }
        if let Some(sigma) = e.blur {
            let id = (sigma.to_bits() as u64) << 32 | 1;
            if n.glass != id {
                // (saturate ×1.5, then blur by sigma: the phone's frost as a blur radius is sigma = .577·r + .5)
                let brush = self.g.os.backdrop_blur(sigma * s, crate::tokens::GLASS_SATURATION)?;
                n.vis.set_brush(Some(&brush))?;
                n.glass = id;
            }
        } else if e.video {
            if let Some(b) = &self.video_brush {
                n.vis.set_brush(Some(b))?;
                n.video = true;
            }
        } else if let Some(img) = &e.image {
            // (a glow, "glow:RRGGBB": the one shared picture of its shape, "glow:mask", filled with its colour)
            let glow = img.url.strip_prefix("glow:").and_then(|hex| u32::from_str_radix(hex, 16).ok());
            let shape;
            let img = match glow {
                Some(_) => { shape = Img { url: "glow:mask".into(), ..img.clone() }; &shape }
                None => img,
            };
            let pic = pic_key(img, w * s, h * s, e.look.radius * s);
            n.image_how = (img.fade_ms, img.contain, glow);
            let have = self.pics.contains_key(&pic);
            let visible = image_viewport.is_none_or(|area| {
                absolute_x < area[2] && absolute_y < area[3] && absolute_x + w > area[0] && absolute_y + h > area[1]
            });
            if have && visible && self.pics.get(&pic).is_some_and(|p| p.strip.is_some()) {
                self.animated_wanted.insert(pic);
            }
            if n.image != Some((pic, have)) {
                show(&mut n)?;
                n.opaque_at = None;
                if have {
                    let faded = n.image.is_some_and(|i| i.0 == pic && !i.1) || fresh;
                    self.put_picture(&mut n, pic, faded)?;
                } else {
                    n.vis.set_brush(None)?;
                }
                n.image = Some((pic, have));
            }
            if !have && visible {
                self.want_pic(pic, img, w, h, e.look.radius);
            }
            if let Some(p) = self.pics.get_mut(&pic) {
                p.used = self.frame;
                e.look.fill = Fill::None;
            }
        } else if let Some(c) = e.look.flat() {
            if n.look != e.look {
                n.vis.set_brush(Some(&self.g.os.solid(c)?))?;
            }
        } else if n.look.flat().is_some() {
            n.vis.set_brush(None)?;
        }

        if e.image.is_none() {
            n.opaque_at = None;
        }

        // The skin: anything drawn, above the base: on a part of a shared surface when it's small (shown 1:1 from
        // there, the same pixels as a surface of its own), else on its own.
        let drawn = !e.look.plain() && e.look.flat().is_none();
        let mut reskinned = false;
        if drawn {
            if n.skin.is_none() || resized || n.look != e.look {
                if n.skin.is_none() || resized {
                    reskinned = true;
                    if let Some(old) = n.skin.take() {
                        n.vis.remove(&old.vis)?;
                    }
                    let v = self.sprite()?;
                    let (pw, ph) = (whole_px(w * s), whole_px(h * s));
                    let (brush, place) = if !e.spin && pw < 512 && ph < 128 {
                        let (page, shelf, x, y, surface) = self.atlas.borrow_mut().take(&self.g, pw, ph)?;
                        let brush = self.g.os.surface_brush(&surface, Fit::At { offset: [-(x as f32), -(y as f32)], scale: [1., 1.] })?;
                        (brush, Place::Shared(Region { atlas: self.atlas.clone(), page, shelf, surface, x, y, w: pw, h: ph }))
                    } else {
                        let surface = self.g.surface(w, h)?;
                        (self.g.os.surface_brush(&surface, Fit::Stretch)?, Place::Own(surface))
                    };
                    v.set_brush(Some(&brush))?;
                    v.set_size(pw as f32, ph as f32)?;
                    n.vis.insert_bottom(&v)?;
                    if e.spin {
                        self.spin(&v, w * s)?;
                    }
                    n.skin = Some(Skin { vis: v, brush, place });
                }
                let look = &e.look;
                n.skin.as_ref().unwrap().paint(&self.g, |c| paint(c, look, w, h))?;
            }
        } else if let Some(old) = n.skin.take() {
            n.vis.remove(&old.vis)?;
        }
        n.look = e.look;
        // a tint: the drawing (white) as the stencil over a colour brush; a new colour animated on that brush, from
        // wherever it is now, so it glides however often it changes
        if let (Some(c), Some(skin)) = (e.tint, n.skin.as_ref()) {
            match &mut n.tint {
                Some((brush, last)) if !reskinned => {
                    if *last != c {
                        self.g.os.animate_colour(brush, &Animation::new(420).key(1., c, Easing::Bezier(crate::tokens::EASE_STANDARD)))?;
                        *last = c;
                    }
                }
                _ => {
                    let brush = self.g.os.color_brush(c)?;
                    skin.vis.set_brush(Some(&self.g.os.masked(&brush, &skin.brush)?))?;
                    n.tint = Some((brush, c));
                }
            }
        } else if e.tint.is_none() {
            n.tint = None;
        }

        // (a node already showing, told to leave: the very node plays its way out, the reverse of how it came in;
        // nothing is built afresh for it)
        if !fresh && leaving && n.out != e.anim {
            show(&mut n)?;
            self.arrive(&n.vis, e.anim, x * s, y * s, w * s, h * s)?;
        }
        n.out = if leaving { e.anim } else { Anim::None };
        if fresh {
            self.arrive(&n.vis, e.anim, x * s, y * s, w * s, h * s)?;
            if e.blink {
                self.blink(&n.vis)?;
            }
            if e.shimmer {
                self.shimmer(&n.vis, w)?;
                n.band = Some(Band { since: self.now, width: w, running: true, stop_at: None });
            }
            if let Some((dy, secs)) = e.drift {
                self.drift(&n.vis, dy, secs)?;
            }
        }

        // Where the children live: straight on the visual, or in a scrolled holder.
        if e.scroll != Scroll::No {
            let st = self.scrolls.entry(key).or_default();
            st.max = if e.scroll == Scroll::Y { (content_h - h).max(0.) } else { (content_w - w).max(0.) };
            let was_pos = st.pos;
            st.pos = st.pos.clamp(0., st.max);
            st.built_at = st.pos;
            let pos = st.pos;
            if n.inner.is_none() {
                let holder = self.g.os.container()?;
                n.vis.insert_top(&holder)?;
                n.inner = Some(holder);
            }
            let inner = n.inner.as_ref().unwrap();
            if fresh || was_pos != pos {
                if e.scroll == Scroll::Y { inner.set_offset(0., -pos * s)? } else { inner.set_offset(-pos * s, 0.)? }
            }
            if e.scroll == Scroll::Y { child_origin.1 -= pos; } else { child_origin.0 -= pos; }
            let area = [absolute_x - 360., absolute_y - 360., absolute_x + w + 360., absolute_y + h + 360.];
            child_view = Some(match viewport {
                Some(v) => [area[0].max(v[0]), area[1].max(v[1]), area[2].min(v[2]), area[3].min(v[3])],
                None => area,
            });
            // Keep the layout margin wider than the image queue. Pictures just ahead of the
            // scroll position may load, while distant rows wait for the viewer to reach them.
            // (a row of cards loads a screen's width ahead of where it's scrolled to)
            let pad = if e.scroll == Scroll::Y { (32., 155.) } else { (w.max(600.), 32.) };
            let area = [absolute_x - pad.0, absolute_y - pad.1, absolute_x + w + pad.0, absolute_y + h + pad.1];
            child_images = Some(match image_viewport {
                Some(v) => [area[0].max(v[0]), area[1].max(v[1]), area[2].min(v[2]), area[3].min(v[3])],
                None => area,
            });
        }

        if e.kids.is_empty() && n.kids.is_empty() { return Ok(n); }
        // Children: matched by key, so a row that moves keeps its visual.
        let ids = t.children(id).map_err(|e| Error::new(e.to_string()))?;
        // (a child without a key is known by its place among the others without one: a keyed one coming or going
        // before it, a page fading out say, mustn't make it a new child, rebuilt and faded in again)
        let mut unkeyed = 0u64;
        let mut plan = Vec::with_capacity(e.kids.len());
        for (kid, cid) in e.kids.into_iter().zip(ids) {
            if kid.key == 0 { unkeyed += 1; }
            let i = unkeyed.wrapping_sub(1);
            if let Some(view) = child_view {
                let layout = t.layout(cid).map_err(|e| Error::new(e.to_string()))?;
                let (left, top) = (child_origin.0 + layout.location.x, child_origin.1 + layout.location.y);
                if left > view[2] || top > view[3] || left + layout.size.width < view[0] || top + layout.size.height < view[1] {
                    continue;
                }
            }
            let kk = if kid.key != 0 { kid.key } else { mix(key, i.wrapping_add(1)) };
            plan.push((kid, cid, kk));
        }
        // (the usual frame: the very same children in the same order, each matched to its own in place; no maps, no
        // visuals moved)
        if plan.len() == n.kids.len() && plan.iter().zip(&n.kids).all(|(p, o)| p.2 == o.key) {
            let old = std::mem::take(&mut n.kids);
            let mut kids = Vec::with_capacity(plan.len());
            for ((kid, cid, kk), prev) in plan.into_iter().zip(old) {
                kids.push(self.reconcile(t, cid, kid, Some(prev), kk, Within { origin: child_origin, viewport: child_view, image_viewport: child_images })?);
            }
            self.settle_bands(&mut kids, w, h)?;
            n.kids = kids;
            n.key = key;
            return Ok(n);
        }
        let order_before: Vec<u64> = n.kids.iter().map(|c| c.key).collect();
        let mut was: HashMap<u64, Node> = n.kids.drain(..).map(|c| (c.key, c)).collect();
        let mut kids = Vec::with_capacity(plan.len());
        for (kid, cid, kk) in plan {
            let prev = was.remove(&kk);
            kids.push(self.reconcile(t, cid, kid, prev, kk, Within { origin: child_origin, viewport: child_view, image_viewport: child_images })?);
        }
        let holder = n.inner.as_ref().unwrap_or(&n.vis);
        let retained: std::collections::HashSet<_> = order_before.iter().copied()
            .filter(|key| !was.contains_key(key)).collect();
        let reordered = order_before.iter().filter(|key| retained.contains(key))
            .ne(kids.iter().map(|kid| &kid.key).filter(|key| retained.contains(key)));
        for (_, gone) in was.drain() {
            holder.remove(&gone.vis)?;
        }
        let mut previous: Option<&Visual> = None;
        for kid in &kids {
            let existing = retained.contains(&kid.key);
            if reordered && existing { holder.remove(&kid.vis)?; }
            if reordered || !existing {
                if let Some(previous) = previous {
                    holder.insert_above(&kid.vis, previous)?;
                } else if let Some(skin) = n.skin.as_ref().filter(|_| n.inner.is_none()) {
                    holder.insert_above(&kid.vis, &skin.vis)?;
                } else {
                    holder.insert_bottom(&kid.vis)?;
                }
            }
            previous = Some(&kid.vis);
        }
        self.settle_bands(&mut kids, w, h)?;
        n.kids = kids;
        n.key = key;
        Ok(n)
    }

    /// A node's opacity changed: eased there over `ease` from wherever it is now (so a change of mind mid-way carries
    /// on smoothly), or set; at nothing and `alone`, taken out of the compositor's work once there.
    fn set_opacity(&mut self, n: &mut Node, opacity: f32, ease: i64, fresh: bool, alone: bool) -> Result<()> {
        if opacity > 0. { show(n)?; }
        if ease > 0 && !fresh {
            self.g.os.animate(&n.vis, Scalar::Opacity, &Animation::new(ease).key(1., opacity, Easing::Bezier(crate::tokens::EASE_OUT)))?;
        } else {
            n.vis.set_opacity(opacity)?;
        }
        n.opacity = opacity;
        if opacity == 0. && alone {
            if ease > 0 && !fresh {
                let at = self.now + std::time::Duration::from_millis(ease as u64);
                n.hide_at = Some(at);
                self.wake_at(at);
            } else {
                n.hidden = true;
                n.vis.set_visible(false)?;
            }
        }
        Ok(())
    }

    /// The hover fades of the elements the pointer came onto or left, played at once on the nodes as they are.
    fn hover_fades(&mut self, over: &HashSet<u64>) -> Result<()> {
        fn walk(kit: &mut Kit, n: &mut Node, over: &HashSet<u64>) -> Result<()> {
            if let Some((key, off, on)) = n.hover_fade {
                if over.contains(&key) {
                    let to = if kit.hover_path.contains(&key) { on } else { off };
                    if n.opacity != to {
                        let (ease, lone) = (n.ease, n.lone);
                        kit.set_opacity(n, to, ease, false, lone)?;
                    }
                }
            }
            for kid in &mut n.kids {
                walk(kit, kid, over)?;
            }
            Ok(())
        }
        self.now = Instant::now();
        let mut tree = self.tree.take();
        let result = match tree.as_mut() {
            Some(root) => walk(self, root, over),
            None => Ok(()),
        };
        self.tree = tree;
        result
    }

    /// Whether the node with this key can be kept as it is (El::retain): it's on screen, and nothing waits for it to be
    /// built again.
    pub fn can_retain(&self, key: u64) -> bool {
        !self.unkept.contains(&key) && self.find(key).is_some()
    }

    /// A kept subtree's pictures marked as in use (never let go while it's kept; one still loading carries on), and
    /// pictures just in put on as a frame would. True if one needs it built again (its node draws a fill the
    /// picture replaces).
    fn keep(&mut self, n: &mut Node) -> Result<bool> {
        let mut again = false;
        match n.image {
            Some((pic, true)) => {
                if let Some(p) = self.pics.get_mut(&pic) { p.used = self.frame; }
            }
            Some((pic, false)) if self.pics.contains_key(&pic) => {
                if n.look.fill != Fill::None || n.glass != 0 || n.video {
                    again = true;
                } else {
                    show(n)?;
                    self.put_picture(n, pic, true)?;
                    n.image = Some((pic, true));
                    self.pics.get_mut(&pic).unwrap().used = self.frame;
                }
            }
            Some((pic, false)) if self.loading.contains_key(&pic) => {
                self.wanted.insert(pic);
            }
            _ => {}
        }
        for kid in &mut n.kids {
            again |= self.keep(kid)?;
        }
        if n.kids.iter().any(|kid| kid.band.is_some()) {
            let (w, h) = (n.w, n.h);
            self.settle_bands(&mut n.kids, w, h)?;
        }
        Ok(again)
    }

    // ---------- animations the compositor plays ----------
    /// How a fresh element arrives (or a leaving copy goes). x, y, w, h in pixels.
    fn arrive(&self, v: &Visual, anim: Anim, x: f32, y: f32, w: f32, h: f32) -> Result<()> {
        use crate::tokens::*;
        let s = self.g.scale;
        let os = &self.g.os;
        let centre = |cx: f32, cy: f32| v.set_centre(w * cx, h * cy);
        let scalar = |prop: Scalar, a: Animation<f32>| os.animate(v, prop, &a);
        let scale = |a: Animation<[f32; 2]>| os.animate_pair(v, Pair::Scale, &a);
        match anim {
            Anim::None => {}
            Anim::Fade(time) => scalar(Scalar::Opacity, curved(EASE_EASE, time, |p| p))?,
            Anim::Rise(dy, time) => {
                scalar(Scalar::Opacity, curved(EASE_EASE, time, |p| p))?;
                scalar(Scalar::OffsetY, curved(EASE_EASE, time, |p| y + dy * s * (1. - p)))?;
            }
            // (every screen the same, as the sections slide: in from the right on the pager's spring; leaving, out to it)
            // (a page opening fades in as it settles from a touch smaller; closing, the same reversed, quicker)
            Anim::Page => {
                centre(0.5, 0.5)?;
                scalar(Scalar::Opacity, curved(EASE_OUT, 240, |p| p))?;
                scale(curved(EASE_OUT, 280, |p| { let k = 0.965 + 0.035 * p; [k, k] }))?;
            }
            Anim::PageOut => {
                centre(0.5, 0.5)?;
                // (the way in, reversed: fading out as it settles back to a touch smaller)
                scalar(Scalar::Opacity, curved(EASE_OUT, 220, |p| 1. - p))?;
                scale(curved(EASE_OUT, 240, |p| { let k = 1. - 0.035 * p; [k, k] }))?;
            }
            Anim::Grow(cx, cy) => {
                centre(cx, cy)?;
                scalar(Scalar::Opacity, spring(SPRING_MENU, |x| (2.2 * x).min(1.)))?;
                scale(spring(SPRING_MENU, move |x| [0.35 + 0.65 * x, 0.2 + 0.8 * x]))?;
                scalar(Scalar::OffsetY, spring(SPRING_MENU, move |x| y - (1. - x) * 10. * s))?;
            }
            Anim::Shrink(cx, cy) => {
                centre(cx, cy)?;
                let t = D_MENU_CLOSE as i64;
                scalar(Scalar::Opacity, curved(EASE_ACCELERATE, t, |p| (2.2 * (1. - p)).min(1.)))?;
                scale(curved(EASE_ACCELERATE, t, move |p| { let x = 1. - p; [0.35 + 0.65 * x, 0.2 + 0.8 * x] }))?;
            }
            Anim::Flash => {
                centre(0.5, 0.5)?;
                scalar(Scalar::Opacity, Animation::new(650).key(0., 0., Easing::Linear).key(0.12, 1., Easing::Bezier(EASE_OUT)).key(1., 0., Easing::Bezier(EASE_STANDARD)))?;
                scale(curved(EASE_OUT, 650, |p| { let k = 0.85 + 0.35 * p; [k, k] }))?;
            }
            Anim::Enter(dx, dy, time) => {
                scalar(Scalar::Opacity, curved(EASE_EASE, time, |p| p))?;
                if dx != 0. { scalar(Scalar::OffsetX, curved(EASE_EASE, time, move |p| x + dx * s * (1. - p)))?; }
                if dy != 0. { scalar(Scalar::OffsetY, curved(EASE_EASE, time, move |p| y + dy * s * (1. - p)))?; }
            }
            Anim::Pop(from, pivot) => {
                v.set_centre(w * 0.5, pivot * s)?;
                scalar(Scalar::Opacity, curved(EASE_EASE, 90, |p| p))?;
                scale(curved(EASE_OUT, 260, move |p| { let k = from + (1. - from) * p; [k, k] }))?;
            }
            Anim::FadeOut(time) => scalar(Scalar::Opacity, curved(EASE_STANDARD, time, |p| 1. - p))?,
            Anim::Turn(time) => {
                centre(0.5, 0.5)?;
                // (still until 70% of the way, turned over by 90%, held there to the end)
                scalar(Scalar::Rotation, Animation::new(time).key(0.7, 0., Easing::Linear).key(0.9, 180., SETTLE).key(1., 180., Easing::Linear).forever())?;
            }
            Anim::Spin(dir) => {
                centre(0.5, 0.5)?;
                scalar(Scalar::Rotation, curved(EASE_EASE, 420, move |p| 360. * dir * p))?;
            }
            Anim::Splash(piece, a) => {
                // the website's clock: the z in over 460 ms; the f dropping from 160 ms on its spring; the name opening
                // out 120 ms after the f settles, over 680 ms. Finely sampled and played by the compositor, which moves
                // them smoothly between the pixels, as moving type should be: never laid out and snapped frame by frame
                let ease = |p: f32| bezier([0.22, 1., 0.36, 1.], p.clamp(0., 1.));
                let (drop_d, drop_k, drop_t) = SPRING_DROP;
                let unfold = 160. + drop_t as f32 + 120.;
                let total = unfold + 680.;
                let open = move |t: f32| if t < unfold { 0. } else { ease((t - unfold) / 680.) };
                let zv = move |t: f32| ease(t / 460.);
                let fv = move |t: f32| if t < 160. { 0. } else { spring_at(drop_d, drop_k, (t - 160.).min(drop_t as f32)) };
                let (bx, by) = (x.round(), y.round());   // (x, y, w and h come here in pixels already)
                match piece {
                    0 => {
                        scalar(Scalar::OffsetX, timeline(SPLASH_DELAY, total, move |t| bx + (a * (1. - open(t)) - 40. * (1. - zv(t))) * s))?;
                        scalar(Scalar::Opacity, timeline(SPLASH_DELAY, total, zv))?;
                    }
                    2 => {
                        scalar(Scalar::OffsetX, timeline(SPLASH_DELAY, total, move |t| bx + a * (1. - open(t)) * s))?;
                        scalar(Scalar::OffsetY, timeline(SPLASH_DELAY, total, move |t| by - (1. - fv(t)) * 46. * s))?;
                        scalar(Scalar::Opacity, timeline(SPLASH_DELAY, total, move |t| fv(t).clamp(0., 1.)))?;
                    }
                    _ => {
                        // (it follows the letter before it, shown from its left edge as the name opens)
                        scalar(Scalar::OffsetX, timeline(SPLASH_DELAY, total, move |t| bx + a * (1. - open(t)) * s))?;
                        scalar(Scalar::Opacity, timeline(SPLASH_DELAY, total, move |t| open(t) * open(t)))?;
                        scalar(Scalar::ClipRight, timeline(SPLASH_DELAY, total, move |t| w * (1. - open(t))))?;
                    }
                }
            }
            Anim::Close(dx, time, fold) => {
                let total = time as f32;
                let open = move |t: f32| bezier([0.22, 1., 0.36, 1.], (1. - t / total).clamp(0., 1.));
                let bx = x.round();
                scalar(Scalar::OffsetX, timeline(0., total, move |t| bx + dx * s * open(t)))?;
                if fold {
                    scalar(Scalar::Opacity, timeline(0., total, move |t| open(t) * open(t)))?;
                    scalar(Scalar::ClipRight, timeline(0., total, move |t| w * (1. - open(t))))?;
                }
            }
            Anim::Shine(i, n) => {
                // (a slice of the band of light: 1.1 s across, 0.4 s rest, again and again, from when the name's open)
                let width = w;
                let band = 0.4 * width;
                let slice = band / n;
                let left = move |t: f32| {
                    let g = if t < 1100. { -0.35 + 1.7 * t / 1100. } else { 1.35 };
                    g * width - band / 2. + slice * i
                };
                let start = SPLASH_DELAY + 160. + SPRING_DROP.2 as f32 + 120. + 680.;
                scalar(Scalar::ClipLeft, looped(1500., start, move |t| left(t).clamp(0., width)))?;
                scalar(Scalar::ClipRight, looped(1500., start, move |t| width - (left(t) + slice).clamp(0., width)))?;
            }
            Anim::FillX(from, time) => {
                centre(0., 0.5)?;
                scale(Animation::new(time.max(1)).key(0., [from.max(0.001), 1.], Easing::Linear).key(1., [1., 1.], Easing::Linear))?;
            }
            Anim::Shift(dx, time) => scalar(Scalar::OffsetX, curved(EASE_STANDARD, time, move |p| x + dx * s * (1. - p)))?,
            Anim::Drop | Anim::DropOut => {
                centre(1., 0.)?;
                let out = anim == Anim::DropOut;
                let f = move |p: f32| if out { 1. - p } else { p };
                scalar(Scalar::Opacity, curved(EASE_EASE, 240, f))?;
                scalar(Scalar::OffsetY, curved(EASE_EASE, 240, move |p| y - 6. * s * (1. - f(p))))?;
                scale(curved(EASE_EASE, 240, move |p| { let k = 0.97 + 0.03 * f(p); [k, k] }))?;
            }
        }
        Ok(())
    }

    fn fade(&self, v: &Visual, time: i64) -> Result<()> {
        if time <= 0 {
            return Ok(());   // (shown at once: the compositor refuses a 0 ms animation)
        }
        self.g.os.animate(v, Scalar::Opacity, &Animation::new(time).key(0., 0., Easing::Linear).key(1., 1., SETTLE))
    }
    /// The caret's blink: on for half of every 1.06 s, off for the rest.
    fn blink(&self, v: &Visual) -> Result<()> {
        let a = Animation::new(1060).key(0.5, 1., Easing::Step).key(0.51, 0., Easing::Step).key(1., 0., Easing::Step).forever();
        self.g.os.animate(v, Scalar::Opacity, &a)
    }
    fn spin(&self, v: &Visual, size: f32) -> Result<()> {
        v.set_centre(size / 2., size / 2.)?;
        self.g.os.animate(v, Scalar::Rotation, &Animation::new(crate::tokens::D_SPINNER as i64).key(1., 360., Easing::Linear).forever())
    }
    /// Pressed in (or let go) on the press spring.
    fn press(&self, v: &Visual, from: f32, to: f32) -> Result<()> {
        self.g.os.animate_pair(v, Pair::Scale, &spring(crate::tokens::SPRING_PRESS, move |x| {
            let k = from + (to - from) * x;
            [k, k]
        }))
    }
    /// The skeleton's band (0.9 of its box wide) sweeping left to right across the box, linear, every 1.6 s.
    fn shimmer(&self, v: &Visual, band: f32) -> Result<()> {
        let s = self.g.scale;
        self.g.os.animate(v, Scalar::OffsetX, &Animation::new(1600).key(0., -band * s, Easing::Linear).key(1., band / 0.9 * s, Easing::Linear).forever())
    }
    fn drift(&self, v: &Visual, dy: f32, secs: f32) -> Result<()> {
        let s = self.g.scale;
        // (up: from where it is to dy; down: from -dy back to where it is, so both start full)
        let (from, to) = if dy < 0. { (0., dy * s) } else { (-dy * s, 0.) };
        self.g.os.animate(v, Scalar::OffsetY, &Animation::new((secs * 1000.) as i64).key(0., from, Easing::Linear).key(1., to, Easing::Linear).forever())
    }
    fn scale_to(&self, v: &Visual, to: f32, time: i64) -> Result<()> {
        self.g.os.animate_pair(v, Pair::Scale, &Animation::new(time).key(1., [to, to], SETTLE))
    }

    // ---------- pictures ----------
    fn want_pic(&mut self, pic: u64, img: &Img, w: f32, h: f32, radius: f32) {
        if img.url.is_empty() || w <= 0. || h <= 0. { return; }
        self.wanted.insert(pic);
        if self.loading.contains_key(&pic) || self.failed_pictures.get(&pic).is_some_and(|at| at.elapsed().as_secs() < 30) {
            return;
        }
        let s = self.g.scale;
        let (pw, ph) = (crate::gfx::whole_px(w * s), crate::gfx::whole_px(h * s));
        let job = crate::images::load(img.clone(), pw, ph, move |app, decoded| {
            if !app.kit.loading.contains_key(&pic) { return; }
            if let Some(decoded) = decoded {
                app.kit.uploads.push_back(Upload { key: pic, pixels: decoded.pixels, frames: decoded.frames, pw, ph, w, h, radius });
                // (put on in the next frame by apply_uploads: nothing else needs building again)
                crate::quiet();
            } else {
                app.kit.loading.remove(&pic);
                if app.kit.failed_pictures.len() >= 256 {
                    app.kit.failed_pictures.retain(|_, at| at.elapsed().as_secs() < 30);
                }
                app.kit.failed_pictures.insert(pic, std::time::Instant::now());
            }
        });
        self.loading.insert(pic, job);
    }

    fn flush_uploads(&mut self) -> Vec<u64> {
        let start = std::time::Instant::now();
        let mut stored = Vec::new();
        while let Some(upload) = self.uploads.pop_front() {
            if self.loading.remove(&upload.key).is_none() { continue; }
            let key = upload.key;
            if self.store_pic(upload).is_ok() {
                stored.push(key);
            }
            if start.elapsed() >= std::time::Duration::from_millis(4) { break; }
        }
        stored
    }

    pub fn has_uploads(&self) -> bool {
        !self.uploads.is_empty()
    }

    /// A picture that's in, on its node: its brush (or a glow's colour masked by it), its fade when it's just
    /// come, and when it's fully in.
    fn put_picture(&mut self, n: &mut Node, pic: u64, faded: bool) -> Result<()> {
        let (fade_ms, contain, glow) = n.image_how;
        // (a node that has already shown this very picture, built again (a card moved into a new wrapper, a row
        // scrolled back to) shows it at once: it doesn't fade in a second time; a glow's layer always fades, being
        // the crossfade itself)
        let faded = faded && (glow.is_some() || self.seen.get(&n.key) != Some(&pic));
        if glow.is_none() {
            if self.seen.len() >= 20_000 {
                self.seen.clear();
            }
            self.seen.insert(n.key, pic);
        }
        let p = &self.pics[&pic];
        if let Some(rgb) = glow {
            let colour = self.g.os.color_brush(0xFF000000 | rgb)?;
            n.vis.set_brush(Some(&self.g.os.masked(&colour, &p.brush)?))?;
        } else {
            n.vis.set_brush(Some(&p.brush))?;
        }
        if faded {
            self.fade(&n.vis, fade_ms)?;
        }
        n.opaque_at = (!contain).then(|| if faded { self.now + std::time::Duration::from_millis(fade_ms.max(0) as u64) } else { self.now });
        Ok(())
    }

    /// Pictures just decoded put straight on the nodes waiting for them, as a frame would, without building the page
    /// again (a few milliseconds' worth a frame, as before). One that needs more (it moves, or its node draws a fill
    /// of its own the picture replaces) leaves it to a whole frame.
    pub fn apply_uploads(&mut self) -> Result<()> {
        self.now = Instant::now();
        let stored = self.flush_uploads();
        if stored.is_empty() { return Ok(()); }
        let stored: HashSet<u64> = stored.into_iter().collect();
        if stored.iter().any(|key| self.pics.get(key).is_some_and(|p| p.strip.is_some())) {
            self.dirty = true;
            return Ok(());
        }
        fn attach(kit: &mut Kit, n: &mut Node, stored: &HashSet<u64>) -> Result<bool> {
            let mut changed = false;
            if let Some((pic, false)) = n.image {
                if stored.contains(&pic) {
                    if n.look.fill != Fill::None || n.glass != 0 || n.video {
                        kit.dirty = true;
                    } else {
                        show(n)?;
                        kit.put_picture(n, pic, true)?;
                        n.image = Some((pic, true));
                        changed = true;
                    }
                }
            }
            let mut kids_changed = false;
            for kid in &mut n.kids {
                kids_changed |= attach(kit, kid, stored)?;
            }
            if kids_changed {
                let (w, h) = (n.w, n.h);
                kit.settle_bands(&mut n.kids, w, h)?;
            }
            Ok(changed)
        }
        let mut tree = self.tree.take();
        let result = match tree.as_mut() {
            Some(root) => attach(self, root, &stored).map(|_| ()),
            None => Ok(()),
        };
        self.tree = tree;
        result?;
        self.trim_pics();
        Ok(())
    }

    fn store_pic(&mut self, upload: Upload) -> Result<()> {
        let Upload { key: pic, pixels, frames, pw, ph, w, h, radius } = upload;
        let s = self.g.scale;
        // An animated picture: every frame drawn once, side by side (a pixel apart, so none ever bleeds into the next)
        // in rows, in one surface; the brush shows one frame's place at a time, moved on by the compositor at each
        // frame's own delay, in step, forever. Nothing is painted again; no timer.
        let (cell_w, cell_h) = (pw + 1, ph + 1);
        let cols = (8192 / cell_w).max(1);
        let strip = frames.filter(|frames| frames.len() > 1 && (frames.len() as u32).div_ceil(cols) * cell_h <= 8192);
        if let Some(frames) = strip {
            let n = frames.len() as u32;
            let (sw, sh) = (cell_w * cols.min(n), cell_h * n.div_ceil(cols));
            let surface = self.g.os.surface(sw, sh)?;
            let place = |i: u32| (i % cols * cell_w, i / cols * cell_h);
            self.g.paint(&surface, |c| {
                for (i, frame) in frames.iter().enumerate() {
                    c.pixels(&frame.pixels, pw, ph, w, h, radius, place(i as u32))?;
                }
                Ok(())
            })?;
            // (shown 1:1 from its top left, scaled as Fill scales a lone frame's surface to the box: the very same pixels)
            let (kx, ky) = (w * s / pw as f32, h * s / ph as f32);
            let at = |i: u32| { let (x, y) = place(i); [-(x as f32) * kx, -(y as f32) * ky] };
            let brush = self.g.os.surface_brush(&surface, Fit::At { offset: at(0), scale: [kx, ky] })?;
            // (each frame from the moment the ones before it have had their delays; held to a hair before the next,
            // then stepped, so a step easing's jump lands on the frame's own time whichever end of its span it takes)
            let total: u128 = frames.iter().map(|f| f.delay.as_millis().max(1)).sum();
            let hair = 0.25 / total as f32;
            let mut a = Animation::new(total as i64).key(0., at(0), Easing::Step);
            let mut since = 0u128;
            for i in 1..n {
                since += frames[i as usize - 1].delay.as_millis().max(1);
                let t = since as f32 / total as f32;
                a = a.key(t - hair, at(i - 1), Easing::Step).key(t, at(i), Easing::Step);
            }
            let a = a.key(1., at(n - 1), Easing::Step).forever();
            self.g.os.animate_brush_offset(&brush, &a)?;
            // (held until it's shown: it plays only where it's on screen, as before)
            self.g.os.play_brush(&brush, false)?;
            let bytes = (sw * sh * 4) as usize;
            self.pic_bytes += bytes;
            self.pics.insert(pic, Pic { brush, bytes, used: self.frame, strip: Some(false) });
            return Ok(());
        }
        let surface = self.g.surface(w, h)?;
        self.g.paint(&surface, |c| c.pixels(&pixels, pw, ph, w, h, radius, (0, 0)))?;
        let brush = self.g.os.surface_brush(&surface, Fit::Stretch)?;
        let bytes = (pw * ph * 4) as usize;
        self.pic_bytes += bytes;
        self.pics.insert(pic, Pic { brush, bytes, used: self.frame, strip: None });
        Ok(())
    }

    /// Animated pictures play where they're on screen and hold where they're not (or while the window's minimised),
    /// carrying on from the same frame when shown again.
    fn play_strips(&mut self) {
        for (key, p) in self.pics.iter_mut() {
            let Some(playing) = p.strip.as_mut() else { continue };
            let want = !self.still && self.animated_wanted.contains(key);
            if *playing == want { continue; }
            if !matches!(self.g.os.play_brush(&p.brush, want), Ok(false)) {
                *playing = want;
            }
        }
    }

    /// The window minimised (true) or back: animated pictures held meanwhile.
    pub fn hold_pictures(&mut self, still: bool) {
        self.still = still;
        self.play_strips();
    }

    /// Past the limit the longest-unused go. Anything on screen was marked this frame and stays.
    fn trim_pics(&mut self) {
        if self.pic_bytes <= PIC_MOST {
            return;
        }
        let mut all: Vec<(u64, u64)> = self.pics.iter().map(|(k, p)| (p.used, *k)).collect();
        all.sort();
        for (used, key) in all {
            if self.pic_bytes <= PIC_MOST * 3 / 4 || used == self.frame {
                break;
            }
            // (one a node on screen still wants is never let go, however old)
            if self.wanted.contains(&key) {
                continue;
            }
            if let Some(p) = self.pics.remove(&key) {
                self.pic_bytes -= p.bytes;
            }
        }
    }

    // ---------- the pointer ----------
    /// Whether a point is over something that takes the pointer (a button, a field, a card).
    pub fn hot_at(&self, x: f32, y: f32) -> bool {
        self.hit(x, y).iter().any(|n| n.hot || n.field.is_some() || n.click.is_some() || n.scroll != Scroll::No && false)
    }
    fn hit(&self, x: f32, y: f32) -> Vec<&Node> {
        fn walk<'a>(n: &'a Node, kit: &Kit, x: f32, y: f32, path: &mut Vec<&'a Node>) -> bool {
            if n.decorative { return false; }
            let (lx, ly) = (x - n.x, y - n.y);
            let inside = lx >= 0. && ly >= 0. && lx < n.w && ly < n.h;
            if n.clip && !inside {
                return false;
            }
            path.push(n);
            let off = if n.scroll != Scroll::No { kit.scroll_of(n.key).pos } else { 0. };
            let (cx, cy) = match n.scroll {
                Scroll::Y => (lx, ly + off),
                Scroll::X => (lx + off, ly),
                Scroll::No => (lx, ly),
            };
            for kid in n.kids.iter().rev() {
                if walk(kid, kit, cx, cy, path) {
                    return true;
                }
            }
            if inside && (n.hot || n.field.is_some() || n.scroll != Scroll::No || n.glass != 0 || n.look.fill != Fill::None) {
                return true;
            }
            path.pop();
            false
        }
        let mut path = Vec::new();
        if let Some(root) = &self.tree {
            walk(root, self, x, y, &mut path);
        }
        path
    }

    /// Whether the pointer's over this element or something in it (noted: the pages are built again when that changes).
    /// Where a node is on the window as last laid out (its scrolling taken into account): x, y, width, height.
    pub fn rect_of(&self, key: u64) -> Option<(f32, f32, f32, f32)> {
        fn walk(n: &Node, kit: &Kit, ox: f32, oy: f32, key: u64) -> Option<(f32, f32, f32, f32)> {
            let (x, y) = (ox + n.x, oy + n.y);
            if n.key == key {
                return Some((x, y, n.w, n.h));
            }
            let off = if n.scroll != Scroll::No { kit.scroll_of(n.key).pos } else { 0. };
            let (cx, cy) = match n.scroll {
                Scroll::Y => (x, y - off),
                Scroll::X => (x - off, y),
                Scroll::No => (x, y),
            };
            n.kids.iter().find_map(|c| walk(c, kit, cx, cy, key))
        }
        self.tree.as_ref().and_then(|t| walk(t, self, 0., 0., key))
    }
    pub fn hover_within(&self, key: u64) -> bool {
        self.asking.borrow_mut().insert(key);
        self.hover_path.contains(&key)
    }
    /// What's under the pointer that takes it (noted: the pages are built again when that changes).
    pub fn hovered(&self) -> u64 {
        self.asking_hover.set(true);
        self.hover
    }

    /// What's under the pointer looked at again (after a redraw: a panel opening over it, a page changing under it).
    pub fn refresh_hover(&mut self) -> bool {
        self.update_hover(false)
    }
    pub fn moved(&mut self, x: f32, y: f32) -> bool {
        self.pointer = (x, y);
        self.update_hover(true)
    }

    fn update_hover(&mut self, update_cursor: bool) -> bool {
        let (key, hand, growing, path_keys) = {
            let path = self.hit(self.pointer.0, self.pointer.1);
            let hit = path.iter().rev().find(|n| n.hot || n.field.is_some());
            let growing = path.iter().rev().find(|n| n.grow != 1.).map_or(0, |n| n.key);
            let hand = match hit {
                Some(n) if n.grab => Cursor::Move,
                Some(n) if n.hand || n.field.is_some() => Cursor::Hand,
                _ => Cursor::Arrow,
            };
            (hit.map_or(0, |n| n.key), hand, growing, path.iter().map(|n| n.key).collect::<Vec<_>>())
        };
        // (the elements the pointer came onto or left: built again only if the pages asked about one of them, else
        // their hover fades played on the nodes as they are)
        let mut over: HashSet<u64> = HashSet::new();
        if self.hover_path != path_keys {
            over.extend(self.hover_path.iter().filter(|k| !path_keys.contains(k)));
            over.extend(path_keys.iter().filter(|k| !self.hover_path.contains(k)));
        }
        self.hover_path = path_keys;
        // (moving a picture about: the move cursor all the way, wherever the pointer goes)
        let hand = if self.dragging.is_some() && self.grabbing { Cursor::Move } else { hand };
        if update_cursor { platform::set_cursor(hand); }
        let changed = (key != self.hover && self.asked_hover) || over.iter().any(|k| self.asked.contains(k));
        if !over.is_empty() {
            let _ = self.hover_fades(&over);
        }
        if growing != self.growing {
            // (in a little slower than out; both from wherever it is, so passing over a card never snaps)
            if let Some(old) = self.find(self.growing) {
                let _ = self.scale_to(&old.vis, 1., 300);
            }
            if let Some(next) = self.find(growing) {
                let _ = self.scale_to(&next.vis, next.grow, 380);
            }
        }
        self.hover = key;
        self.growing = growing;
        self.dirty |= changed;
        changed
    }

    fn find(&self, key: u64) -> Option<&Node> {
        fn walk(n: &Node, key: u64) -> Option<&Node> {
            if n.key == key {
                return Some(n);
            }
            n.kids.iter().find_map(|c| walk(c, key))
        }
        if key == 0 {
            return None;
        }
        self.tree.as_ref().and_then(|t| walk(t, key))
    }

    /// The press: returns what to run.
    pub fn down(&mut self, x: f32, y: f32) -> Option<(DragCb, f32)> {
        self.pointer = (x, y);
        let (key, sink, vis, field, drag) = {
            let path = self.hit(x, y);
            let field = path.iter().rev().find(|n| n.field.is_some()).map(|n| {
                let mut ox = 0.;
                for p in &path {
                    ox += p.x;
                    if std::ptr::eq(*p, *n) {
                        break;
                    }
                }
                let spec = n.field.as_ref().unwrap();
                (n.key, x - ox - n.look_pad() + spec.offset, spec.style.clone(), spec.secret)
            });
            // (the nearest thing under the pointer that does something; a picture that only grows on hover inside a
            // button mustn't take its click)
            let hot = path.iter().rev().find(|n| n.hot && (n.click.is_some() || n.drag.is_some())).or_else(|| path.iter().rev().find(|n| n.hot));
            let drag = hot.and_then(|n| {
                n.drag.clone().map(|d| {
                    let (mut ox, mut oy) = (0., 0.);
                    for p in &path {
                        ox += p.x;
                        oy += p.y;
                        if std::ptr::eq(*p, *n) {
                            break;
                        }
                    }
                    if n.drag_up { (d, oy, n.h, true) } else { (d, ox, n.w, false) }
                })
            });
            (hot.map(|n| n.key).unwrap_or(0), hot.map(|n| n.sink).unwrap_or(1.), hot.map(|n| n.vis.clone()), field, drag)
        };
        self.pressed = key;
        if sink != 1. {
            if let Some(v) = &vis {
                let _ = self.press(v, 1., sink);
            }
        }
        match field {
            Some((fk, fx, style, secret)) => {
                self.focus = fk;
                let e = self.edits.entry(fk).or_default();
                let shown = if secret { "•".repeat(e.text.encode_utf16().count()) } else { e.text.clone() };
                let at = self.g.hit_char(&shown, &style, fx.max(0.));
                let e = self.edits.get_mut(&fk).unwrap();
                e.caret = at;
                e.anchor = at;
                self.dirty = true;
            }
            None => {
                if self.focus != 0 {
                    self.focus = 0;
                    self.dirty = true;
                }
            }
        }
        if let Some((d, o, l, up)) = drag {
            self.dragging = Some((d.clone(), o, l, up));
            let grabbing = self.hit(x, y).iter().any(|n| n.grab);
            self.grabbing = grabbing;
            return Some((d, along(x, y, o, l, up)));
        }
        None
    }

    pub fn dragged(&self, x: f32, y: f32) -> Option<(DragCb, f32)> {
        self.dragging.as_ref().map(|(d, o, l, up)| (d.clone(), along(x, y, *o, *l, *up)))
    }

    pub fn up(&mut self, x: f32, y: f32) -> (Option<Cb>, Option<(DragCb, f32)>) {
        self.pointer = (x, y);
        let released = self.dragging.take().map(|(d, o, l, up)| (d, along(x, y, o, l, up)));
        if let Some(n) = self.find(self.pressed) {
            if n.sink != 1. {
                let (v, sink) = (n.vis.clone(), n.sink);
                let _ = self.press(&v, sink, 1.);
            }
        }
        let click = {
            let path = self.hit(x, y);
            path.iter().find(|n| n.key == self.pressed && self.pressed != 0).and_then(|n| n.click.clone())
        };
        self.pressed = 0;
        (click, released)
    }

    /// What a right-click here does: the nearest thing under the pointer with a `right_click`.
    pub fn context_at(&mut self, x: f32, y: f32) -> Option<Cb> {
        self.pointer = (x, y);
        self.hit(x, y).iter().rev().find_map(|n| n.context.clone())
    }

    /// Smooth wheel: one animation aimed at the new position, played by the compositor, restarted by each notch.
    pub fn wheel(&mut self, delta: f32, x: f32, y: f32, sideways: bool) -> Result<()> {
        let want = if sideways { Scroll::X } else { Scroll::Y };
        let target = {
            let path = self.hit(x, y);
            path.iter().rev().find(|n| n.scroll == want && self.scroll_of(n.key).max > 0.).map(|n| (n.key, n.inner.clone()))
        };
        let Some((key, Some(inner))) = target else { return Ok(()) };
        let s = self.g.scale;
        let st = self.scrolls.entry(key).or_default();
        let pos = (st.pos - delta).clamp(0., st.max);
        if st.pos == pos { return Ok(()); }
        st.pos = pos;
        // (a long page builds only what's near the screen: past this far, it's built again further on; near the top
        // every notch, as headers turn solid and the glow eases off there. A row of cards: past this far too (what's
        // built reaches 360 past its edges, its pictures a whole width), or as its arrows come or go at either end)
        let ends = |at: f32| (at > 4., st.max - at > 4.);
        let rebuild = match want {
            Scroll::X => (st.pos - st.built_at).abs() >= 120. || ends(st.pos) != ends(st.built_at),
            _ => (st.pos - st.built_at).abs() >= 120. || st.pos.min(st.built_at) < 900.,
        };
        if rebuild {
            self.dirty = true;
        }
        let pos = st.pos;
        let a = Animation::new(if want == Scroll::Y { 140 } else { 210 }).key(1., -pos * s, SETTLE);
        self.g.os.animate(&inner, if want == Scroll::Y { Scalar::OffsetY } else { Scalar::OffsetX }, &a)?;
        if want == Scroll::X && !rebuild {
            // (what's under the pointer now: a frame only if that changed)
            self.update_hover(false);
        }
        Ok(())
    }

    /// Move a scroll by `by` at once, with no animation (content above changed size: what's on screen stays put).
    pub fn shift_scroll(&mut self, key: u64, by: f32) {
        let s = self.g.scale;
        let st = self.scrolls.entry(key).or_default();
        st.pos = (st.pos + by).max(0.);
        st.built_at = st.pos;
        let pos = st.pos;
        if let Some(inner) = self.find(key).and_then(|node| node.inner.as_ref()) {
            let _ = inner.stop(Scalar::OffsetY);
            let _ = inner.set_offset(0., -pos * s);
        }
        self.dirty = true;
    }

    pub fn reset_scroll(&mut self, key: u64) {
        self.scrolls.entry(key).or_default().pos = 0.;
        if let Some(inner) = self.find(key).and_then(|node| node.inner.as_ref()) {
            let _ = inner.stop(Scalar::OffsetY);
            let _ = inner.set_offset(0., 0.);
        }
        self.dirty = true;
    }

    /// Straight to a place, nothing played (a page coming back from under the player to what it was showing).
    pub fn scroll_now(&mut self, key: u64, pos: f32) {
        let st = self.scrolls.entry(key).or_default();
        st.pos = pos.clamp(0., st.max);
        let pos = st.pos;
        if let Some(inner) = self.find(key).and_then(|node| node.inner.as_ref()) {
            let _ = inner.stop(Scalar::OffsetY);
            let _ = inner.set_offset(0., -pos * self.g.scale);
        }
        self.dirty = true;
    }

    /// Glides to a place over 420 ms (the arrows on a row, the reader's keys).
    pub fn glide(&mut self, key: u64, pos: f32) -> Result<()> {
        let st = self.scrolls.entry(key).or_default();
        st.pos = pos.clamp(0., st.max);
        let pos = st.pos;
        self.dirty = true;
        let s = self.g.scale;
        if let Some(n) = self.find(key) {
            if let Some(inner) = &n.inner {
                let prop = if n.scroll == Scroll::Y { Scalar::OffsetY } else { Scalar::OffsetX };
                self.g.os.animate(inner, prop, &Animation::new(420).key(1., -pos * s, SETTLE))?;
            }
        }
        Ok(())
    }

    // ---------- typing ----------
    pub fn tab(&mut self, backwards: bool) {
        fn fields(node: &Node, found: &mut Vec<u64>) {
            if node.field.is_some() { found.push(node.key); }
            for child in &node.kids { fields(child, found); }
        }
        let mut order = Vec::new();
        if let Some(root) = &self.tree { fields(root, &mut order); }
        if order.is_empty() { return; }
        let index = match order.iter().position(|key| *key == self.focus) {
            Some(index) if backwards => (index + order.len() - 1) % order.len(),
            Some(index) => (index + 1) % order.len(),
            None if backwards => order.len() - 1,
            None => 0,
        };
        self.focus = order[index];
        self.dirty = true;
    }

    pub fn field_spec(&self) -> Option<(Option<Cb>, Option<Cb>)> {
        self.find(self.focus).and_then(|n| n.field.as_ref()).map(|f| (f.change.clone(), f.submit.clone()))
    }
    pub fn focused(&mut self) -> Option<&mut Edit> {
        let f = self.focus;
        if f == 0 {
            return None;
        }
        Some(self.edits.entry(f).or_default())
    }
}

/// A keyframe animation sampled at `n` even steps over `ms`: `first` at 0, then `at(p)` at each step p (to 1), linear
/// between them, so any shape plays exactly. Every curve, spring and timeline below is one of these.
fn sampled<V: Copy>(n: u32, ms: i64, first: V, at: impl Fn(f32) -> V) -> Animation<V> {
    let mut a = Animation::new(ms).key(0., first, Easing::Linear);
    for i in 1..=n {
        let p = i as f32 / n as f32;
        a = a.key(p, at(p), Easing::Linear);
    }
    a
}

/// A spring (damping, stiffness, settle ms) played as keyframes sampled from it: `value(v)` for its position v (0 -> 1).
fn spring<V: Copy>(spring: (f32, f32, u32), value: impl Fn(f32) -> V) -> Animation<V> {
    let (d, k, t) = spring;
    sampled(30, t as i64, value(0.), |p| value(if p >= 1. { 1. } else { crate::tokens::spring_at(d, k, p * t as f32) }))
}

/// A curve played from 0 to 1 over `time`: `value(v)` sampled at 24 points along the bezier `b`.
fn curved<V: Copy>(b: [f32; 4], time: i64, value: impl Fn(f32) -> V) -> Animation<V> {
    sampled(24, time, value(0.), |p| value(bezier(b, p)))
}

/// A value following `value` over `total` ms (its argument: ms from the start), held at its first value for `delay` ms
/// first (the splash, waiting for the window to be up).
fn timeline(delay: f32, total: f32, value: impl Fn(f32) -> f32) -> Animation<f32> {
    sampled(120, total as i64, value(0.), |p| value(total * p)).delay(delay as i64)
}

/// The same, again and again every `period` ms, beginning after `delay` ms (its first value held until then).
fn looped(period: f32, delay: f32, value: impl Fn(f32) -> f32) -> Animation<f32> {
    sampled(90, period as i64, value(0.), |p| value(period * p)).delay(delay as i64).forever()
}

/// Back in the compositor's work (something to show again, or something about to play on it).
fn show(n: &mut Node) -> Result<()> {
    if n.hidden || n.hide_at.is_some() {
        n.hidden = false;
        n.hide_at = None;
        n.vis.set_visible(n.band.as_ref().is_none_or(|b| b.running))?;
    }
    Ok(())
}

impl Node {
    /// (fields put their text 18 in from the edge unless they say otherwise)
    fn look_pad(&self) -> f32 {
        self.kids.first().map(|k| k.x).unwrap_or(0.)
    }
}

/// How far along a drag is: across from its left, or up from its foot.
fn along(x: f32, y: f32, origin: f32, length: f32, up: bool) -> f32 {
    let length = length.max(1.);
    (if up { 1. - (y - origin) / length } else { (x - origin) / length }).clamp(0., 1.)
}

fn pic_key(i: &Img, w: f32, h: f32, r: f32) -> u64 {
    let mut s = std::collections::hash_map::DefaultHasher::new();
    (&i.url, &i.fallback, w.to_bits(), h.to_bits(), r.to_bits(), (i.gray.to_bits(), i.bright.to_bits(), i.lift.to_bits()), i.crop, i.contain, i.crop_y.to_bits(), i.centred, i.animated, i.shade).hash(&mut s);
    s.finish()
}

/// A text to measure: the El's own (it stays put until the layout's computed).
type Measured = Option<*const (String, TextStyle)>;

/// The El's layout into the tree: its style moved in (nothing reads it after), its text pointed at.
fn add(t: &mut TaffyTree<Measured>, e: &mut El) -> NodeId {
    let kids: Vec<NodeId> = e.kids.iter_mut().map(|k| add(t, k)).collect();
    let style = std::mem::take(&mut e.style);
    if kids.is_empty() {
        t.new_leaf_with_context(style, e.look.text.as_ref().map(|text| text as *const _)).unwrap()
    } else {
        t.new_with_children(style, &kids).unwrap()
    }
}

/// A fill as paint for a `w` by `h` box (its points are fractions of the box).
fn paint_of(f: &Fill, w: f32, h: f32) -> Option<Paint<'_>> {
    match f {
        Fill::None => None,
        Fill::Solid(c) => Some(Paint::Solid(*c)),
        Fill::Linear(x0, y0, x1, y1, stops) => Some(Paint::Linear { from: (x0 * w, y0 * h), to: (x1 * w, y1 * h), stops }),
        Fill::Radial(cx, cy, r, stops) => Some(Paint::Radial { centre: (cx * w, cy * h), radius: (r * w, r * w), stops }),
    }
}

fn paint(c: &mut Canvas<'_>, l: &Look, w: f32, h: f32) -> Result<()> {
    let whole = Rect { x: 0., y: 0., w, h };
    let radius = l.radius.min(h / 2.).min(w / 2.);
    if let Some(p) = paint_of(&l.fill, w, h) {
        c.fill_rounded(whole, radius, &p)?;
    }
    if l.border > 0. {
        if let Some(p) = paint_of(&l.border_fill, w, h) {
            let i = l.border / 2.;
            c.stroke_rounded(Rect { x: i, y: i, w: w - l.border, h: h - l.border }, (radius - i).max(0.), &p, l.border)?;
        }
    }
    if let Some(line) = l.top_line {
        // (a 1 px line along the top, inside the rounded corners)
        c.push_clip(Rect { x: 0., y: 0., w, h: 1. }, true);
        let drawn = c.fill_rounded(whole, radius, &Paint::Solid(line));
        c.pop_clip();
        drawn?;
    }
    if let Some(place) = l.rank.filter(|p| (1..=10).contains(p)) {
        // the figure as given: an outlined ring, white fading to nothing across it, at its own opacity, in a box
        // the full height, against the right (where the poster is), cut off at the box's right edge
        let f = &crate::rank_digits::FIGURES[place - 1];
        let k = h / crate::rank_digits::HEIGHT;
        let box_w = f.width * k;
        let ox = w - box_w;
        let white = ((f.opacity * 255.).round() as u32) << 24 | 0xFFFFFF;
        let fade = Fill::Linear((ox + f.from * k) / w, 0., (ox + f.to * k) / w, 0., vec![(0., white), (1., 0x00FFFFFF)]);
        if let Some(fade) = paint_of(&fade, w, h) {
            c.push_clip(Rect { x: ox.max(0.), y: 0., w: box_w.min(w), h }, true);
            let drawn = c.fill_path(f.path, f.even_odd, k, ox, 0., &fade);
            c.pop_clip();
            drawn?;
        }
    }
    if let Some((name, size, colour)) = l.icon {
        c.icon(name, (w - size) / 2., (h - size) / 2., size, colour)?;
    }
    if let Some((s, t)) = &l.text {
        if let Some((width, colour)) = l.outline {
            // (an outline: the text drawn around itself in the outline's colour, then over it; one layout for all sixteen)
            let ring: Vec<(f32, f32)> = (0..16).map(|i| {
                let a = i as f32 * std::f32::consts::TAU / 16.;
                (a.cos() * width, a.sin() * width)
            }).collect();
            c.text(s, t, w + 1., h + 1., colour, &ring)?;
        }
        // (drawn with room past its box: a bold letter's overhang, then rounding, cut "lix" to "l…"; only text that
        // really doesn't fit gets its ellipsis)
        c.text(s, t, w + 1. + if t.centre { 0. } else { t.size * 0.15 }, h + 1., t.color, &[(0., 0.)])?;
    }
    if let Some((stroke, colour, sweep)) = l.arc {
        // An arc with round caps from the top, clockwise: the spinner (turned by the compositor) or a ring's progress.
        let r = (w - stroke) / 2.;
        let (cx, cy) = (w / 2., h / 2.);
        if sweep >= 0.999 {
            c.stroke_circle((cx, cy), r, colour, stroke)?;
        } else if sweep > 0. {
            let end = -std::f32::consts::FRAC_PI_2 + sweep * std::f32::consts::TAU;
            c.stroke_arc((cx, cy - r), (cx + r * end.cos(), cy + r * end.sin()), r, sweep > 0.5, colour, stroke)?;
        }
    }
    Ok(())
}
