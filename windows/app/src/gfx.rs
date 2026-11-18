//! Text styles, and the kit's handle on the OS's drawing: its scale, and texts' sizes, measured once and remembered.
use crate::platform::{Canvas, Compositor, Graphics, Result, Surface};
use std::cell::RefCell;
use std::collections::HashMap;

/// Geist only: regular 400, medium 500, bold = SemiBold 600 (the phone's bold).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Font {
    Sans,
    Medium,
    Bold,
}

#[derive(Clone, PartialEq, Debug)]
pub struct TextStyle {
    pub font: Font,
    pub size: f32,
    pub color: u32,
    pub line: f32,
    pub spacing: f32,
    pub lines: u32,
    pub centre: bool,
}

/// How many whole pixels a length of `px` pixels needs. A length meant to be whole that arithmetic left a hair over
/// (1,500.0009) isn't a pixel more: that pixel came out nearly see-through at the picture's foot, a seam under it.
pub fn whole_px(px: f32) -> u32 {
    ((px - 1. / 64.).ceil() as u32).max(1)
}

/// Measured texts by key: (width, height).
type TextSizes = HashMap<u64, (f32, f32)>;

pub struct Gfx {
    pub os: Graphics,
    /// Pixels per layout unit.
    pub scale: f32,
    /// Texts' sizes: the ones measured lately, and the generation before (kept until the new one fills, never all
    /// thrown away at once).
    measured: RefCell<(TextSizes, TextSizes)>,
}

impl Gfx {
    pub fn new(os: Graphics, scale: f32) -> Gfx {
        Gfx { os, scale, measured: Default::default() }
    }

    /// A surface for a box `w` by `h` (layout units), in whole pixels.
    pub fn surface(&self, w: f32, h: f32) -> Result<Surface> {
        let s = self.scale;
        self.os.surface(whole_px(w * s), whole_px(h * s))
    }

    /// The surface cleared and drawn at the scale.
    pub fn paint(&self, surface: &Surface, draw: impl FnOnce(&mut Canvas<'_>) -> Result<()>) -> Result<bool> {
        self.os.paint(surface, self.scale, draw)
    }

    /// The size a text takes in a given width: asked for over and over while laying out, so remembered.
    pub fn measure(&self, text: &str, t: &TextStyle, w: f32) -> (f32, f32) {
        use std::hash::{Hash, Hasher};
        let mut k = std::collections::hash_map::DefaultHasher::new();
        (text, t.font, t.size.to_bits(), t.line.to_bits(), t.spacing.to_bits(), t.lines, w.to_bits()).hash(&mut k);
        let key = k.finish();
        {
            let mut cache = self.measured.borrow_mut();
            if let Some(m) = cache.0.get(&key) {
                return *m;
            }
            if let Some(m) = cache.1.get(&key).copied() {
                cache.0.insert(key, m);
                return m;
            }
        }
        let one = if t.line > 0. { t.line } else { t.size * 1.3 };
        let m = self
            .os
            .text_size(text, t, w)
            .map(|(width, height)| {
                let height = if t.lines > 0 { height.min(one * t.lines as f32 + 1.) } else { height };
                // (tight letter spacing takes its last gap off the measure, and the text would be cut: give it back)
                (width.ceil() + 1. + (-t.spacing).max(0.), height.ceil())
            })
            .unwrap_or((0., one));
        let mut cache = self.measured.borrow_mut();
        if cache.0.len() >= 8000 {
            cache.1 = std::mem::take(&mut cache.0);
        }
        cache.0.insert(key, m);
        m
    }

    /// Where the caret sits before a character, for text fields.
    pub fn caret_x(&self, text: &str, t: &TextStyle, at: usize) -> f32 {
        self.os.caret_x(text, t, at).unwrap_or(0.)
    }

    /// Which character a point along a text falls on, for text fields.
    pub fn hit_char(&self, text: &str, t: &TextStyle, x: f32) -> usize {
        self.os.hit_char(text, t, x).unwrap_or(0)
    }
}
