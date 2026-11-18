//! The seam between the desktop app and the OS under it (decision 0009: the Mac app is this code on its own backend).
//! Everything above it is shared: the pages, the design system, the kit's layout, diffing and hit-testing, and every
//! animation's curve and keyframes. Each OS implements the traits and functions here once, in its own module, as
//! thinly as it can: what it's given is plain values (pixels, ARGB colours, keyframes), so the OSes can't drift.
//!
//! - [`Compositor`]: the visual tree, brushes, surfaces drawn on through a [`Draw`], animations, text measuring.
//! - [`Layer`]: one visual's properties and children.
//! - [`Shell`]: the window, its input, clipboard and timers; [`Event`]s come to the app through [`Callbacks`].
//! - [`Decoder`]: a picture decoded, cropped and scaled.
//! - [`Loader`]: a shared library (libmpv).
//! - `video`, `install` and the free functions (`alert`, `set_cursor`, `utc_offset_seconds`, `os_name`...): the
//!   player's surface, putting the app on and off the computer, small system questions.
#[cfg(target_os = "macos")]
mod mac;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use self::mac::{
    alert, ctrl_down, free_bytes, install, os_name, set_cursor, shift_down, utc_offset_seconds, video, Brush, Canvas,
    ColorBrush, Graphics, Library, Picture, Restore, Surface, Visual, Waker, Window, MODEL,
};
#[cfg(windows)]
pub use self::windows::{
    alert, ctrl_down, free_bytes, install, os_name, set_cursor, shift_down, utc_offset_seconds, video, Brush, Canvas,
    ColorBrush, Graphics, Library, Picture, Restore, Surface, Visual, Waker, Window, MODEL,
};

use crate::gfx::TextStyle;
use std::ffi::c_void;
use std::path::Path;

/// What went wrong below the seam: the OS's own words, and whether the GPU went away (everything is drawn again).
#[derive(Debug, Clone)]
pub struct Error {
    message: String,
    device_lost: bool,
}

impl Error {
    /// An error in these words.
    pub fn new(message: impl Into<String>) -> Error {
        Error { message: message.into(), device_lost: false }
    }
    /// An error that means the GPU device was lost or reset (a Mac's drawing is the CPU's: never there).
    #[cfg(windows)]
    pub fn lost_device(message: impl Into<String>) -> Error {
        Error { message: message.into(), device_lost: true }
    }
    /// The GPU device was lost or reset: drawing starts over on a new one.
    pub fn device_lost(&self) -> bool {
        self.device_lost
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// The seam's result.
pub type Result<T> = std::result::Result<T, Error>;

// ---------- animation ----------

/// How a keyframe is reached from the one before it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Easing {
    Linear,
    /// Holds the last value and jumps at the keyframe's time.
    Step,
    /// CSS `cubic-bezier(x1, y1, x2, y2)`.
    Bezier([f32; 4]),
}

/// A property's animation, played by the OS compositor on its own clock. Keyframes are at progress 0 to 1; one with
/// no keyframe at 0 starts from the property's value as it's started.
#[derive(Clone, Debug, PartialEq)]
pub struct Animation<V> {
    pub frames: Vec<(f32, V, Easing)>,
    pub duration_ms: i64,
    /// Waits this long first, the first keyframe's value held meanwhile.
    pub delay_ms: i64,
    /// Plays again and again.
    pub forever: bool,
}

impl<V> Animation<V> {
    /// No keyframes yet, over `duration_ms`.
    pub fn new(duration_ms: i64) -> Animation<V> {
        Animation { frames: Vec::new(), duration_ms, delay_ms: 0, forever: false }
    }
    /// A keyframe at `at` (0 to 1), reached on `easing`.
    pub fn key(mut self, at: f32, value: V, easing: Easing) -> Animation<V> {
        self.frames.push((at, value, easing));
        self
    }
    /// Begins after `ms`, its first value held until then.
    pub fn delay(mut self, ms: i64) -> Animation<V> {
        self.delay_ms = ms;
        self
    }
    /// Repeats for good.
    pub fn forever(mut self) -> Animation<V> {
        self.forever = true;
        self
    }
}

/// A number on a visual that can be animated.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scalar {
    OffsetX,
    OffsetY,
    Opacity,
    /// Degrees clockwise, about the centre point.
    Rotation,
    /// The inset clip's left and right edges (nothing happens on a visual without one).
    ClipLeft,
    ClipRight,
}

/// A pair of numbers on a visual that can be animated.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pair {
    /// Its place in its parent, in pixels.
    Offset,
    /// Its scale across and down, about the centre point.
    Scale,
}

/// How a visual's content is cut to its box.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Clip {
    None,
    /// To its box; the edges can be moved in (`Scalar::ClipLeft`, `ClipRight`).
    Inset,
    /// To a rounded box this size (pixels).
    Rounded { w: f32, h: f32, radius: f32 },
}

/// How a drawn surface is laid on the visual it fills.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fit {
    /// Stretched to the visual's size.
    Stretch,
    /// Unstretched from its top left, scaled by `scale` and moved by `offset` (pixels): a part of a bigger surface.
    At { offset: [f32; 2], scale: [f32; 2] },
}

// ---------- drawing ----------

/// A box in a surface's own units: left, top, width and height.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// What a shape is filled or stroked with. Gradient stops are (position, ARGB), mixed in sRGB and clamped at the ends.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Paint<'a> {
    Solid(u32),
    Linear { from: (f32, f32), to: (f32, f32), stops: &'a [(f32, u32)] },
    Radial { centre: (f32, f32), radius: (f32, f32), stops: &'a [(f32, u32)] },
}

/// Drawing into a surface (the units are the surface's scaled ones: the kit's layout units).
pub trait Draw {
    fn fill_rounded(&mut self, r: Rect, radius: f32, paint: &Paint) -> Result<()>;
    /// A line of `width` along the rounded box (centred on its edge).
    fn stroke_rounded(&mut self, r: Rect, radius: f32, paint: &Paint, width: f32) -> Result<()>;
    /// Whatever's drawn next is kept inside `r` until `pop_clip`, its edge smooth or on whole pixels.
    fn push_clip(&mut self, r: Rect, smooth: bool);
    fn pop_clip(&mut self);
    /// An SVG path (even-odd or non-zero), scaled by `k` and moved by (dx, dy), filled.
    fn fill_path(&mut self, d: &str, even_odd: bool, k: f32, dx: f32, dy: f32, paint: &Paint) -> Result<()>;
    /// One of the app's icons (`icons`), drawn in a `size` box at (x, y).
    fn icon(&mut self, name: &'static str, x: f32, y: f32, size: f32, argb: u32) -> Result<()>;
    /// Text laid out in a `w` by `h` box, drawn once at each of `at` (an outline draws it round itself).
    fn text(&mut self, text: &str, style: &TextStyle, w: f32, h: f32, argb: u32, at: &[(f32, f32)]) -> Result<()>;
    fn stroke_circle(&mut self, centre: (f32, f32), radius: f32, argb: u32, width: f32) -> Result<()>;
    /// A clockwise arc with round caps from `from` to `to` on a circle of `radius`.
    fn stroke_arc(&mut self, from: (f32, f32), to: (f32, f32), radius: f32, large: bool, argb: u32, width: f32) -> Result<()>;
    /// Decoded premultiplied BGRA pixels (pw by ph) fitted to a `w` by `h` rounded box, `at` (whole pixels) along.
    #[allow(clippy::too_many_arguments)]
    fn pixels(&mut self, pixels: &[u8], pw: u32, ph: u32, w: f32, h: f32, radius: f32, at: (u32, u32)) -> Result<()>;
}

/// Something an SVG path is traced into (`icons::trace`).
pub trait PathSink {
    fn begin(&mut self, x: f32, y: f32);
    fn line(&mut self, x: f32, y: f32);
    fn cubic(&mut self, c1: (f32, f32), c2: (f32, f32), to: (f32, f32));
    fn quad(&mut self, c: (f32, f32), to: (f32, f32));
    /// SVG's arc: radii, rotation (degrees), which way round, to `to`.
    fn arc(&mut self, radii: (f32, f32), rotation: f32, large: bool, clockwise: bool, to: (f32, f32));
    fn end(&mut self, closed: bool);
}

// ---------- the compositor ----------

/// One visual: a box in the tree with its own place, size, opacity, brush and children.
pub trait Layer {
    /// Its place in its parent (pixels).
    fn set_offset(&self, x: f32, y: f32) -> Result<()>;
    fn set_size(&self, w: f32, h: f32) -> Result<()>;
    /// The point it scales and turns about (pixels from its top left).
    fn set_centre(&self, x: f32, y: f32) -> Result<()>;
    fn set_rotation(&self, degrees: f32) -> Result<()>;
    fn set_opacity(&self, opacity: f32) -> Result<()>;
    /// Out of the compositor's work altogether while it's hidden.
    fn set_visible(&self, visible: bool) -> Result<()>;
    /// Scaled by `k` from its top left, then moved by (dx, dy) pixels.
    fn set_transform(&self, dx: f32, dy: f32, k: f32) -> Result<()>;
    /// What fills it (a sprite only).
    fn set_brush(&self, brush: Option<&Brush>) -> Result<()>;
    /// An animation on it stopped where it is.
    fn stop(&self, prop: Scalar) -> Result<()>;
    /// A playing animation moved on to `progress` (0 to 1) of its run.
    fn seek(&self, prop: Scalar, progress: f32) -> Result<()>;
    fn insert_top(&self, child: &Visual) -> Result<()>;
    fn insert_bottom(&self, child: &Visual) -> Result<()>;
    fn insert_above(&self, child: &Visual, sibling: &Visual) -> Result<()>;
    fn remove(&self, child: &Visual) -> Result<()>;
    fn remove_all(&self) -> Result<()>;
    fn has_children(&self) -> Result<bool>;
}

/// The OS compositor and the drawing and text under it: one per window.
pub trait Compositor: Sized {
    /// The compositor on `window`, with the app's fonts from `fonts` (Geist).
    fn new(window: &Window, fonts: &Path) -> Result<Self>;
    /// The tree's root, filling the window.
    fn root(&self) -> &Visual;
    /// A visual that shows a brush.
    fn sprite(&self) -> Result<Visual>;
    /// A visual that only holds others.
    fn container(&self) -> Result<Visual>;
    fn clip(&self, visual: &Visual, clip: Clip) -> Result<()>;
    fn solid(&self, argb: u32) -> Result<Brush>;
    /// A colour that can be animated (shown through `masked`).
    fn color_brush(&self, argb: u32) -> Result<ColorBrush>;
    fn surface_brush(&self, surface: &Surface, fit: Fit) -> Result<Brush>;
    /// `colour` shown through `mask`'s alpha.
    fn masked(&self, colour: &ColorBrush, mask: &Brush) -> Result<Brush>;
    /// What's behind, saturated by `saturation` then blurred by `sigma` (pixels).
    fn backdrop_blur(&self, sigma: f32, saturation: f32) -> Result<Brush>;
    /// The player's picture.
    fn video_brush(&self, video: video::Surface) -> Result<Brush>;
    fn animate(&self, visual: &Visual, prop: Scalar, animation: &Animation<f32>) -> Result<()>;
    fn animate_pair(&self, visual: &Visual, prop: Pair, animation: &Animation<[f32; 2]>) -> Result<()>;
    fn animate_colour(&self, brush: &ColorBrush, animation: &Animation<u32>) -> Result<()>;
    /// A surface brush's offset animated (an animated picture's frames, side by side in its surface).
    fn animate_brush_offset(&self, brush: &Brush, animation: &Animation<[f32; 2]>) -> Result<()>;
    /// That offset animation held or played on from where it was; false if the brush has none.
    fn play_brush(&self, brush: &Brush, playing: bool) -> Result<bool>;
    /// A transparent surface of whole pixels to draw on.
    fn surface(&self, pw: u32, ph: u32) -> Result<Surface>;
    /// The surface cleared and drawn, its units scaled by `scale`. False if the device is gone (nothing drawn).
    fn paint(&self, surface: &Surface, scale: f32, draw: impl FnOnce(&mut Canvas<'_>) -> Result<()>) -> Result<bool>;
    /// One part of a surface (pixels `x, y`, `w` by `h`, and the pixel round it) cleared and drawn as `paint` draws a
    /// surface that size; nothing outside it touched.
    #[allow(clippy::too_many_arguments)]
    fn paint_in(&self, surface: &Surface, x: u32, y: u32, w: u32, h: u32, scale: f32, draw: impl FnOnce(&mut Canvas<'_>) -> Result<()>) -> Result<bool>;
    /// The GPU device replaced after it was lost: every surface must be drawn again.
    fn replace_device(&mut self) -> Result<()>;
    /// A text's width (trailing spaces included) and height laid out `width` wide.
    fn text_size(&self, text: &str, style: &TextStyle, width: f32) -> Result<(f32, f32)>;
    /// Where the caret stands before UTF-16 unit `at`.
    fn caret_x(&self, text: &str, style: &TextStyle, at: usize) -> Result<f32>;
    /// The UTF-16 unit nearest `x` along the text.
    fn hit_char(&self, text: &str, style: &TextStyle, x: f32) -> Result<usize>;
}

// ---------- the window ----------

/// What the window tells the app. Positions are in pixels from the window's top left.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Event {
    /// Work sent from another thread is waiting (`Waker::wake`).
    Wake,
    Resized { minimised: bool },
    /// The frame timer came due.
    Frame,
    /// The slow timer (a few times a second).
    Tick,
    PointerMoved { x: f32, y: f32 },
    PointerLeft,
    PointerDown { x: f32, y: f32 },
    PointerUp { x: f32, y: f32 },
    RightUp { x: f32, y: f32 },
    /// The wheel: 120 a notch, positive away from the viewer; sideways when `horizontal`.
    Wheel { x: f32, y: f32, delta: f32, horizontal: bool, ctrl: bool },
    /// A UTF-16 unit typed (a pair comes as two).
    Char(u16),
    /// A key pressed (`key`).
    Key(u16),
    /// Asked to close: the app has its last word, then the window goes.
    Close,
}

/// How the app hears from its window: plain functions, called on the UI thread.
#[derive(Clone, Copy)]
pub struct Callbacks {
    /// An event: true if the app took it (else the OS does what it does by default).
    pub event: fn(Event) -> bool,
    /// Whether a point on the window (layout units) drags the window: its top strip, where nothing's clickable.
    pub drags: fn(f32, f32) -> bool,
    /// After each message: a frame drawn or asked for.
    pub idle: fn(),
}

/// Which window to open.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Opening {
    /// The setup's small window (installing or uninstalling), not the app's.
    pub setup: bool,
    pub uninstalling: bool,
    /// The setup's and the bootstrapper's size (layout units).
    pub small: (f32, f32),
}

/// The title bar's buttons, asked of the OS (handled after the click that asks).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowCommand {
    Minimise,
    Maximise,
    Restore,
    Close,
}

/// The pointer's look.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cursor {
    Arrow,
    Hand,
    /// Something dragged about.
    Move,
    /// None at all: the player with its controls away (the next move of the mouse brings one back).
    None,
}

/// The app's one window: its top strip is the app's, with the title bar's buttons drawn by the app or, where the OS
/// keeps its own over that strip (the Mac's), by the OS.
pub trait Shell: Copy + Sized {
    /// The app draws the title bar's buttons itself.
    const OWN_BUTTONS: bool;
    /// Opened hidden; None if the app (or setup) is already open, which is brought to the front instead.
    fn open(opening: Opening, callbacks: Callbacks) -> Result<Option<Self>>;
    /// The OS's messages handled until the window's gone.
    fn run(&self);
    /// Pixels per layout unit.
    fn scale(&self) -> f32;
    /// The inside's size in pixels.
    fn client_size(&self) -> (i32, i32);
    fn is_maximised(&self) -> bool;
    fn set_icon(&self, ico: &Path);
    fn show(&self);
    fn to_front(&self);
    /// Kept hidden until `show_held` (the update check answers first).
    fn hold_hidden(&self);
    fn is_held_hidden(&self) -> bool;
    /// Shown maximised if it was held hidden; true if it was.
    fn show_held(&self) -> bool;
    fn hide(&self);
    /// Closing asked for (the app hears `Event::Close`).
    fn close(&self);
    fn command(&self, command: WindowCommand);
    /// Made the bootstrapper: restored, small, in the middle of the screen (asked, done after this turn).
    fn bootstrap(&self);
    /// The OS's own animations for minimising, maximising and restoring.
    fn set_transitions(&self, on: bool);
    /// Over the whole screen; what to restore it to after.
    fn enter_full_screen(&self) -> Option<Restore>;
    fn leave_full_screen(&self, restore: &Restore);
    /// Waits for the screen's next refresh.
    fn wait_for_refresh(&self);
    /// The frame timer started (about a frame from now); false if it couldn't be.
    fn start_frame_timer(&self) -> bool;
    fn stop_frame_timer(&self);
    /// The slow timer started, for good.
    fn start_ticks(&self);
    /// Wakes this window's thread from any thread.
    fn waker(&self) -> Waker;
    /// The clipboard's text, as UTF-16.
    fn clipboard(&self) -> Option<Vec<u16>>;
    fn set_clipboard(&self, text: &[u16]);
    /// A file chosen by the viewer among `kind` (named so) matching `patterns` ("*.jpg;*.png").
    fn pick_file(&self, kind: &str, patterns: &str) -> Option<String>;
}

/// Something that can wake the UI thread from anywhere.
pub trait Wake: Copy + Send + Sync {
    /// The window's told `Event::Wake`; false if it couldn't be.
    fn wake(&self) -> bool;
}

/// The keys `Event::Key` brings: the numbers Windows gives them (virtual-key codes), the same on every OS. A shortcut
/// held with Ctrl on Windows is held with Cmd on a Mac (`ctrl_down`).
pub mod key {
    pub const BACK: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const RETURN: u16 = 0x0D;
    pub const ESCAPE: u16 = 0x1B;
    pub const SPACE: u16 = 0x20;
    pub const PAGE_UP: u16 = 0x21;
    pub const PAGE_DOWN: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const DELETE: u16 = 0x2E;
    /// The digits and letters are their ASCII capitals.
    pub const ZERO: u16 = 0x30;
    pub const A: u16 = 0x41;
    pub const C: u16 = 0x43;
    pub const F: u16 = 0x46;
    pub const J: u16 = 0x4A;
    pub const K: u16 = 0x4B;
    pub const L: u16 = 0x4C;
    pub const M: u16 = 0x4D;
    pub const N: u16 = 0x4E;
    pub const V: u16 = 0x56;
    pub const X: u16 = 0x58;
    pub const PAD_ZERO: u16 = 0x60;
    pub const PAD_PLUS: u16 = 0x6B;
    pub const PAD_MINUS: u16 = 0x6D;
    pub const F11: u16 = 0x7A;
    /// The main keys' = / + and - / _.
    pub const PLUS: u16 = 0xBB;
    pub const MINUS: u16 = 0xBD;
}

// ---------- pictures and libraries ----------

/// How a picture's resized.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resample {
    /// Averaging (small pictures: sharp and quick).
    Fant,
    /// High-quality cubic.
    Cubic,
}

/// A picture being decoded: each step makes a new one, the pixels read at the end.
pub trait Decoder: Sized {
    /// The first frame of a JPEG, PNG, GIF or BMP.
    fn decode(data: &[u8]) -> Result<Self>;
    /// Straight RGBA (or RGB, without `alpha`) rows, `w` by `h`.
    fn from_pixels(w: u32, h: u32, pixels: &[u8], alpha: bool) -> Option<Self>;
    fn size(&self) -> Result<(u32, u32)>;
    /// A JPEG read straight at 1/2, 1/4 or 1/8 by its decoder: the smallest that still covers `w` by `h` once
    /// cropped to fill. None when that wouldn't help.
    fn smaller(&self, w: u32, h: u32) -> Option<Self>;
    fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Result<Self>;
    fn resize(&self, w: u32, h: u32, how: Resample) -> Result<Self>;
    /// Its pixels as premultiplied BGRA into `out`, `stride` bytes a row.
    fn read(&self, stride: u32, out: &mut [u8]) -> Result<()>;
}

/// A shared library loaded at run time.
pub trait Loader: Sized {
    fn load(name: &str) -> Result<Self>;
    /// A function's address in it.
    fn symbol(&self, name: &str) -> Result<*const c_void>;
}
