//! A chapter's own still, for a chapter the server has none for: a real scene from it, not a white speech panel. A few
//! of its pages are looked at, a wide window slid down each, and the busiest window kept (the most pixels that are
//! neither paper-white nor ink-black). The apps decode the pages, crop the window this picks and keep it as a file.

use crate::downloads::chapter_folder;

/// The widest a page is decoded to be judged (its height in proportion); a narrower one stays as it is.
pub const WIDTH: u32 = 480;
/// The JPEG quality a still is kept at.
pub const QUALITY: u8 = 84;
/// A window at least this busy is a good scene: no page further is looked at.
const GOOD: f32 = 0.55;
/// Pixels sampled across and down a window.
const ACROSS: u32 = 24;
const DOWN: u32 = 14;

/// Where a chapter's own still is kept, as [`crate::screen::DiskOp`] names paths.
pub fn file(chapter: &str) -> String {
    format!("manhwa/thumbs2/{}.jpg", chapter_folder(chapter))
}

/// The size a page `width` by `height` is decoded at to be judged.
pub fn fit(width: u32, height: u32) -> (u32, u32) {
    if width <= WIDTH {
        return (width, height);
    }
    (WIDTH, ((u64::from(height) * u64::from(WIDTH) / u64::from(width.max(1))) as u32).max(1))
}

/// Part of a page, as rows of pixels from its top, at the size it was judged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub top: u32,
    pub height: u32,
}

/// Which pages are looked at, in order: a third, half, two thirds and a quarter of the way in (on the device all four,
/// fetched only three, as each costs a download).
pub fn tries(pages: usize, saved: bool) -> Vec<usize> {
    if pages == 0 {
        return vec![];
    }
    let mut out: Vec<usize> = vec![];
    for i in [pages / 3, pages / 2, pages * 2 / 3, pages / 4] {
        let i = i.min(pages - 1);
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out.truncate(if saved { 4 } else { 3 });
    out
}

/// Whether a pixel is picture: neither near-white paper nor near-black ink (lit in between, or coloured and not dark).
fn picture(p: [u8; 3]) -> bool {
    let max = p.iter().copied().max().unwrap_or(0);
    let min = p.iter().copied().min().unwrap_or(0);
    let v = f32::from(max) / 255.;
    let s = if max > 0 { f32::from(max - min) / f32::from(max) } else { 0. };
    (0.12..=0.93).contains(&v) || s > 0.18 && v > 0.12
}

/// How much of a window is picture: the share of a grid of its pixels that are.
fn busy(rgba: &[u8], width: u32, top: u32, height: u32) -> f32 {
    let mut seen = 0u32;
    for x in 0..ACROSS {
        for y in 0..DOWN {
            let px = x * (width - 1) / (ACROSS - 1);
            let py = top + y * (height - 1) / (DOWN - 1);
            let at = ((py * width + px) * 4) as usize;
            if rgba.get(at..at + 3).is_some_and(|c| picture([c[0], c[1], c[2]])) {
                seen += 1;
            }
        }
    }
    seen as f32 / (ACROSS * DOWN) as f32
}

/// A page's windows (as wide as it, 16:9, a third of one apart), each with how busy it is.
fn windows(rgba: &[u8], width: u32, height: u32) -> Vec<(Window, f32)> {
    if width == 0 || height == 0 || rgba.len() < (width as usize) * (height as usize) * 4 {
        return vec![];
    }
    let high = height.min(width * 9 / 16).max(1);
    let step = (high / 3).max(1);
    let mut out = vec![];
    let mut top = 0;
    while top + high <= height {
        out.push((Window { top, height: high }, busy(rgba, width, top, high)));
        top += step;
    }
    out
}

/// A still being picked: ask [`Still::next`] for a page, decode it at [`fit`], hand it to [`Still::looked`], and crop
/// the window it gives (the best so far) until `next` says there's nothing more to look at.
#[derive(Clone, Debug, Default)]
pub struct Still {
    tries: Vec<usize>,
    at: usize,
    best: Option<f32>,
}

impl Still {
    /// A chapter of `pages` pages; `saved`: they're on the device.
    pub fn new(pages: usize, saved: bool) -> Self {
        Still { tries: tries(pages, saved), at: 0, best: None }
    }

    /// The page to look at next; None once the still is picked (or there's none to pick).
    pub fn next(&self) -> Option<usize> {
        if self.best.is_some_and(|b| b >= GOOD) {
            return None;
        }
        self.tries.get(self.at).copied()
    }

    /// The page [`Still::next`] gave, decoded as straight RGBA (None: it wouldn't come or decode). Some: its window to
    /// crop and keep, in place of any before (the busiest yet).
    pub fn looked(&mut self, rgba: Option<&[u8]>, width: u32, height: u32) -> Option<Window> {
        self.at += 1;
        let mut found = None;
        for (window, score) in windows(rgba?, width, height) {
            if self.best.is_none_or(|b| score > b) {
                self.best = Some(score);
                found = Some(window);
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page `w` wide: white paper, with a coloured scene in rows `scene`.
    fn page(w: u32, h: u32, scene: std::ops::Range<u32>) -> Vec<u8> {
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for _ in 0..w {
                out.extend_from_slice(if scene.contains(&y) { &[180, 60, 40, 255] } else { &[255, 255, 255, 255] });
            }
        }
        out
    }

    #[test]
    fn the_pages_looked_at() {
        assert_eq!(tries(12, false), vec![4, 6, 8]);
        assert_eq!(tries(12, true), vec![4, 6, 8, 3]);
        assert_eq!(tries(1, true), vec![0]);
        assert_eq!(tries(2, true), vec![0, 1]);
        assert!(tries(0, false).is_empty());
        assert_eq!(fit(960, 3000), (480, 1500));
        assert_eq!(fit(400, 3000), (400, 3000));
    }

    #[test]
    fn the_busiest_window_is_kept() {
        let mut still = Still::new(12, false);
        assert_eq!(still.next(), Some(4));
        // (all paper: kept for want of better, and the next page looked at)
        let blank = page(48, 200, 0..0);
        assert_eq!(still.looked(Some(&blank), 48, 200), Some(Window { top: 0, height: 27 }));
        assert_eq!(still.next(), Some(6));
        // (one that won't decode is passed over)
        assert_eq!(still.looked(None, 0, 0), None);
        assert_eq!(still.next(), Some(8));
        let scene = page(48, 200, 100..140);
        let w = still.looked(Some(&scene), 48, 200).expect("the scene");
        assert!(w.top >= 100 && w.top + w.height <= 140, "{w:?}");
        // (a good scene: no further page)
        assert_eq!(still.next(), None);
    }
}
