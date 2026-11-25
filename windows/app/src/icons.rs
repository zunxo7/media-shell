//! The app's icons: the Android-style vector files, and an SVG path reader that traces them (and any path) into the
//! OS's geometry.
use crate::platform::PathSink;

macro_rules! icons { ($($n:literal),*) => { fn source(name: &str) -> &'static str { match name { $($n => include_str!(concat!("../assets/icons/ic_", $n, ".xml")),)* _ => "" } } } }
icons!("alert", "book_open", "chevron_bold", "cloud_off", "done", "skip_back", "trash", "arrow_left", "captions", "check", "chevron_down", "chevron_left", "chevron_right", "clapperboard", "download", "eye", "eye_off", "film", "flag", "history", "house", "info", "laptop", "log_out", "maximize", "minimize", "pause", "pencil", "play", "rotate_ccw", "rotate_cw", "search", "settings", "skip_forward", "smartphone", "sparkles", "tv", "user_round", "volume_1", "volume_2", "volume_x", "x", "plus", "minus", "win_x", "win_minus", "win_square", "win_copy", "gauge", "panel_open", "panel_close", "languages", "arrow_left_right", "hourglass", "heart", "copy");

fn attr<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let at = s.find(&format!("android:{name}=\""))? + name.len() + 10;
    Some(&s[at..at + s[at..].find('"')?])
}

/// One path of an icon: its SVG path, whether it's filled, and its stroke width (0 for none).
pub struct Part {
    pub d: &'static str,
    pub filled: bool,
    pub stroke: f32,
}

/// An icon's paths, in the order they're drawn.
pub fn parts(name: &str) -> Vec<Part> {
    let mut out = Vec::new();
    for part in source(name).split("<path").skip(1) {
        let Some(d) = attr(part, "pathData") else { continue };
        let filled = attr(part, "fillColor").is_some_and(|c| !c.starts_with("#00"));
        let stroke = if attr(part, "strokeColor").is_some_and(|c| !c.starts_with("#00")) { attr(part, "strokeWidth").and_then(|w| w.parse().ok()).unwrap_or(1.75) } else { 0. };
        out.push(Part { d, filled, stroke });
    }
    out
}

struct Reader<'a> {
    s: &'a [u8],
    i: usize,
}
impl Reader<'_> {
    fn skip(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b',' | b'\n') {
            self.i += 1;
        }
    }
    fn num(&mut self) -> f32 {
        self.skip();
        let start = self.i;
        let mut dot = false;
        if self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+') {
            self.i += 1;
        }
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c == b'.' {
                if dot {
                    break;
                }
                dot = true;
            } else if (c == b'e' || c == b'E') && self.i > start {
                // (an exponent: "6.44338e-07")
                self.i += 1;
                if self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+') {
                    self.i += 1;
                }
                continue;
            } else if !c.is_ascii_digit() {
                break;
            }
            self.i += 1;
        }
        std::str::from_utf8(&self.s[start..self.i]).unwrap_or("0").parse().unwrap_or(0.)
    }
    /// An arc's flags may be written with no gap: "a1 1 0 00.5 2".
    fn flag(&mut self) -> bool {
        self.skip();
        let on = self.i < self.s.len() && self.s[self.i] == b'1';
        self.i += 1;
        on
    }
}

/// An SVG path's figures traced into `sink`.
pub fn trace(d: &str, sink: &mut impl PathSink) {
    let mut r = Reader { s: d.as_bytes(), i: 0 };
    let (mut x, mut y, mut sx, mut sy) = (0f32, 0f32, 0f32, 0f32);
    let (mut cx, mut cy) = (0f32, 0f32); // the last control point, for S
    let mut open = false;
    let mut cmd = b'M';
    loop {
        r.skip();
        if r.i >= r.s.len() {
            break;
        }
        if r.s[r.i].is_ascii_alphabetic() {
            cmd = r.s[r.i];
            r.i += 1;
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (x, y) } else { (0., 0.) };
        let mut curve = false;
        // (a figure closed with Z may carry on with no M: it starts where the last one closed)
        if !open && !matches!(cmd, b'M' | b'm') {
            sink.begin(x, y);
            open = true;
        }
        match cmd.to_ascii_uppercase() {
            b'M' => {
                if open {
                    sink.end(false);
                }
                x = ox + r.num();
                y = oy + r.num();
                sx = x;
                sy = y;
                sink.begin(x, y);
                open = true;
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                x = ox + r.num();
                y = oy + r.num();
                sink.line(x, y);
            }
            b'H' => {
                x = ox + r.num();
                sink.line(x, y);
            }
            b'V' => {
                y = oy + r.num();
                sink.line(x, y);
            }
            b'C' | b'S' => {
                let (x1, y1) = if cmd.eq_ignore_ascii_case(&b'C') { (ox + r.num(), oy + r.num()) } else { (2. * x - cx, 2. * y - cy) };
                let (x2, y2) = (ox + r.num(), oy + r.num());
                let (ex, ey) = (ox + r.num(), oy + r.num());
                sink.cubic((x1, y1), (x2, y2), (ex, ey));
                cx = x2;
                cy = y2;
                x = ex;
                y = ey;
                curve = true;
            }
            b'Q' => {
                let (x1, y1) = (ox + r.num(), oy + r.num());
                let (ex, ey) = (ox + r.num(), oy + r.num());
                sink.quad((x1, y1), (ex, ey));
                x = ex;
                y = ey;
            }
            b'A' => {
                let (rx, ry, rot) = (r.num(), r.num(), r.num());
                let (large, sweep) = (r.flag(), r.flag());
                let (ex, ey) = (ox + r.num(), oy + r.num());
                sink.arc((rx, ry), rot, large, sweep, (ex, ey));
                x = ex;
                y = ey;
            }
            b'Z' => {
                sink.end(true);
                open = false;
                x = sx;
                y = sy;
            }
            _ => break,
        }
        if !curve {
            cx = x;
            cy = y;
        }
    }
    if open {
        sink.end(false);
    }
}
