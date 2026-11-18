//! The little pictures over a seek bar: a YouTube storyboard's grid, or the frames a title page's sprite sheet lists.

use super::youtube::Board;

/// Where one little picture is: its sheet and its spot on it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Thumb {
    pub url: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// A frame of a sprite sheet and the stretch of the video it stands for.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub start: f64,
    pub end: f64,
    pub thumb: Thumb,
}

/// A video's pictures for its seek bar.
#[derive(Clone, Debug, PartialEq)]
pub enum Preview {
    Board { board: Board },
    Frames { frames: Vec<Frame> },
}

/// Where a title page's sprite sheet is read (through the server, which knows the source).
pub fn sprites_path(sprite: &str) -> String {
    format!("/api/ary/sprites?url={}", super::sources::enc(sprite))
}

/// The frames a sprite sheet's WebVTT lists ("image.jpg#xywh=0,0,160,90"); `base` makes its addresses whole.
pub fn frames(vtt: &str, base: &str) -> Vec<Frame> {
    let stamp = |s: &str| s.trim().split(':').try_fold(0., |t, n| Some(t * 60. + n.parse::<f64>().ok()?));
    vtt.replace('\r', "")
        .split("\n\n")
        .filter_map(|block| {
            let mut lines = block.lines();
            let (a, b) = lines.find_map(|s| s.split_once("-->"))?;
            let (image, coords) = lines.next()?.trim().split_once("#xywh=")?;
            let n: Vec<u32> = coords.split(',').filter_map(|n| n.parse().ok()).collect();
            let &[x, y, w, h] = n.as_slice() else { return None };
            Some(Frame { start: stamp(a)?, end: stamp(b)?, thumb: Thumb { url: crate::api::url(base, image), x, y, w, h } })
        })
        .collect()
}

/// The picture for the moment `at`.
pub fn thumb(preview: &Preview, at: f64) -> Option<Thumb> {
    match preview {
        Preview::Board { board: b } => {
            let frame = ((at / b.interval) as i64).clamp(0, b.count - 1);
            let per = b.cols * b.rows;
            let pos = frame % per;
            let name = if b.name.contains("$M") { (frame / per).to_string() } else { b.name.clone() };
            Some(Thumb {
                url: b.template.replace("$M", &name),
                x: ((pos % b.cols) * b.w) as u32,
                y: ((pos / b.cols) * b.h) as u32,
                w: b.w as u32,
                h: b.h as u32,
            })
        }
        Preview::Frames { frames } => {
            frames.iter().find(|f| f.start <= at && at < f.end).or_else(|| frames.last()).map(|f| f.thumb.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprite_frames_and_board_cells() {
        let vtt = "WEBVTT\n\n00:00.000 --> 00:10.000\n/s/a.jpg#xywh=0,0,160,90\n\n00:10.000 --> 00:20.000\n/s/a.jpg#xywh=160,0,160,90\n";
        let found = frames(vtt, "https://host.invalid");
        assert_eq!(found.len(), 2);
        let p = Preview::Frames { frames: found };
        assert_eq!(thumb(&p, 12.).map(|t| (t.x, t.url)), Some((160, "https://host.invalid/s/a.jpg".into())));
        let board = Board { template: "https://i/M$M.jpg".into(), w: 160, h: 90, count: 50, cols: 5, rows: 5, interval: 2., name: "M$M".into() };
        let t = thumb(&Preview::Board { board }, 60.).unwrap_or_default();
        assert_eq!((t.url.as_str(), t.x, t.y), ("https://i/M1.jpg", 0, 90));
    }
}
