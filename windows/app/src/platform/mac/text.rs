//! Text with Core Text on the bundled Geist files, laid out as DirectWrite lays it out for the Windows app: lines
//! broken to the width (or kept whole), the same uniform line height and baseline (80% down), letter spacing after
//! every character, centring, and a trailing "…" where text runs out of its lines or width.
use crate::gfx::{Font, TextStyle};
use crate::platform::{Error, Result};
use objc2_core_foundation::{CFAttributedString, CFBoolean, CFDictionary, CFNumber, CFRange, CFRetained, CFString, CFType, CGFloat, CGPoint};
use objc2_core_graphics::{CGContext, CGDataProvider, CGFont};
use objc2_core_text::{
    kCTFontAttributeName, kCTForegroundColorFromContextAttributeName, kCTKernAttributeName, CTFont, CTLine, CTLineTruncationType, CTTypesetter,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

/// One laid-out line: where it sits across (centring) and its baseline down from the layout's top.
pub(super) struct Line {
    pub line: CFRetained<CTLine>,
    pub x: f64,
    pub baseline: f64,
}

/// A text laid out in a box.
pub(super) struct Layout {
    pub lines: Vec<Line>,
    /// Across, trailing spaces included.
    pub width: f64,
    pub height: f64,
}

/// The app's three weights, each made at a size once.
pub(super) struct Text {
    faces: HashMap<Font, CFRetained<CGFont>>,
    fonts: RefCell<HashMap<(Font, u32), CFRetained<CTFont>>>,
}

/// A font file's weight (its OS/2 table's usWeightClass), read from the table directory.
fn weight(data: &[u8]) -> Option<u16> {
    let tables = u16::from_be_bytes(data.get(4..6)?.try_into().ok()?) as usize;
    (0..tables).find_map(|i| {
        let record = data.get(12 + i * 16..28 + i * 16)?;
        (&record[..4] == b"OS/2").then(|| {
            let at = u32::from_be_bytes(record[8..12].try_into().ok()?) as usize;
            Some(u16::from_be_bytes(data.get(at + 4..at + 6)?.try_into().ok()?))
        })?
    })
}

impl Text {
    /// The fonts in `dir`, known by their weight: 400 regular, 500 medium, 600 the app's bold (as on Windows).
    pub fn new(dir: &Path) -> Result<Text> {
        let mut faces = HashMap::new();
        for entry in std::fs::read_dir(dir).map_err(|e| Error::new(e.to_string()))? {
            let path = entry.map_err(|e| Error::new(e.to_string()))?.path();
            let Ok(data) = std::fs::read(&path) else { continue };
            let font = match weight(&data) {
                Some(400) => Font::Sans,
                Some(500) => Font::Medium,
                Some(600) => Font::Bold,
                _ => continue,
            };
            let provider = CGDataProvider::with_cf_data(Some(&objc2_core_foundation::CFData::from_bytes(&data)));
            if let Some(face) = provider.and_then(|p| CGFont::with_data_provider(&p)) {
                faces.insert(font, face);
            }
        }
        if faces.len() < 3 {
            return Err(Error::new(format!("Geist's fonts aren't all in {}", dir.display())));
        }
        Ok(Text { faces, fonts: Default::default() })
    }

    fn font(&self, font: Font, size: f32) -> CFRetained<CTFont> {
        let key = (font, (size * 16.) as u32);
        if let Some(f) = self.fonts.borrow().get(&key) {
            return f.clone();
        }
        let f = unsafe { CTFont::with_graphics_font(&self.faces[&font], size as CGFloat, std::ptr::null(), None) };
        self.fonts.borrow_mut().insert(key, f.clone());
        f
    }

    /// The text with its font and spacing, its colour taken from the context it's drawn in.
    fn attributed(&self, text: &str, t: &TextStyle) -> CFRetained<CFAttributedString> {
        let font = self.font(t.font, t.size);
        let kern = CFNumber::new_f64(t.spacing as f64);
        let mut keys: Vec<&CFString> = unsafe { vec![kCTFontAttributeName, kCTForegroundColorFromContextAttributeName] };
        let mut values: Vec<&CFType> = vec![font.as_ref(), CFBoolean::new(true).as_ref()];
        if t.spacing != 0. {
            keys.push(unsafe { kCTKernAttributeName });
            values.push(kern.as_ref());
        }
        let attributes = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
        unsafe { CFAttributedString::new(None, Some(&CFString::from_str(text)), Some(attributes.as_opaque())) }
            .unwrap_or_else(|| unsafe { CFAttributedString::new(None, Some(&CFString::from_str("")), None) }.expect("an empty string"))
    }

    /// One line's height and its baseline down from its top: the style's own line height (baseline 80% down, as
    /// DirectWrite's uniform spacing has it), or the font's ascent, descent and gap.
    fn metrics(&self, t: &TextStyle) -> (f64, f64) {
        if t.line > 0. {
            return (t.line as f64, t.line as f64 * 0.8);
        }
        let font = self.font(t.font, t.size);
        let (ascent, descent, leading) = unsafe { (font.ascent(), font.descent(), font.leading()) };
        (ascent + descent + leading, ascent)
    }

    /// The text laid out `w` wide and `h` tall.
    pub fn layout(&self, text: &str, t: &TextStyle, w: f32, h: f32) -> Layout {
        let (w, h) = (w.max(1.) as f64, h.max(1.) as f64);
        let (line_h, baseline) = self.metrics(t);
        let attributed = self.attributed(text, t);
        let setter = unsafe { CTTypesetter::with_attributed_string(&attributed) };
        let length = text.encode_utf16().count() as isize;
        // (one line only: still broken at a newline, as DirectWrite's no-wrap is)
        let wrap_at = if t.lines == 1 { 1e9 } else { w };
        let mut ranges = Vec::new();
        let mut start = 0;
        while start < length {
            let count = unsafe { setter.suggest_line_break(start, wrap_at) }.max(1);
            ranges.push(CFRange::new(start, count));
            start += count;
        }
        if ranges.is_empty() {
            ranges.push(CFRange::new(0, 0));
        }
        let trim = t.lines > 0;
        // (trimmed, only the lines that fit the box: the last of them ends in "…" if more text was to come)
        let fits = if trim { ((h + 0.01) / line_h).floor().max(1.) as usize } else { usize::MAX };
        let shown = ranges.len().min(fits);
        let ellipsis = || unsafe { CTLine::with_attributed_string(&self.attributed("\u{2026}", t)) };
        let mut lines = Vec::with_capacity(shown);
        let mut width: f64 = 0.;
        for (i, range) in ranges.iter().take(shown).enumerate() {
            let cut = trim && i + 1 == shown && shown < ranges.len();
            let mut line = if cut {
                unsafe { setter.line(CFRange::new(range.location, length - range.location)) }
            } else {
                unsafe { setter.line(*range) }
            };
            let full = typographic_width(&line);
            let inked = full - unsafe { line.trailing_whitespace_width() };
            if trim && (cut || inked > w + 0.5) {
                if let Some(short) = unsafe { line.truncated_line(w, CTLineTruncationType::End, Some(&ellipsis())) } {
                    line = short;
                }
            }
            let full = typographic_width(&line);
            let inked = full - unsafe { line.trailing_whitespace_width() };
            let x = if t.centre { ((w - inked) / 2.).max(0.) } else { 0. };
            width = width.max(full);
            lines.push(Line { line, x, baseline: i as f64 * line_h + baseline });
        }
        Layout { lines, width, height: shown as f64 * line_h }
    }

    /// The text as one line, unbroken (a field's).
    fn single(&self, text: &str, t: &TextStyle) -> CFRetained<CTLine> {
        unsafe { CTLine::with_attributed_string(&self.attributed(text, t)) }
    }

    /// Where the caret stands before UTF-16 unit `at`.
    pub fn caret_x(&self, text: &str, t: &TextStyle, at: usize) -> f64 {
        unsafe { self.single(text, t).offset_for_string_index(at as isize, std::ptr::null_mut()) }
    }

    /// The UTF-16 unit nearest `x` along the text.
    pub fn hit_char(&self, text: &str, t: &TextStyle, x: f32) -> usize {
        let i = unsafe { self.single(text, t).string_index_for_position(CGPoint::new(x as f64, 0.)) };
        i.max(0) as usize
    }

    /// The laid-out text drawn with its top left at (x, y) in the context's colour, in a context whose y runs down.
    pub fn draw(&self, layout: &Layout, cg: &CGContext, x: f64, y: f64) {
        CGContext::set_text_matrix(Some(cg), objc2_core_foundation::CGAffineTransform { a: 1., b: 0., c: 0., d: -1., tx: 0., ty: 0. });
        for line in &layout.lines {
            CGContext::set_text_position(Some(cg), x + line.x, y + line.baseline);
            unsafe { line.line.draw(cg) };
        }
    }
}

fn typographic_width(line: &CTLine) -> f64 {
    unsafe { line.typographic_bounds(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) }
}
