//! What downloads make of HLS playlists ([`crate::hls`] reads them): the level and sound a download takes, and the
//! playlists it plays from on the device (its pieces as files there).

use crate::hls::{self, Level};
use crate::quality;

/// The first 3 or 4 digits followed by "p" in `text` ("…_1080p…"): 0 when there are none.
fn height_in_name(text: &str) -> i64 {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let n = i - start;
            if (3..=4).contains(&n) && matches!(bytes.get(i), Some(b'p' | b'P')) {
                return text[start..i].parse().unwrap_or(0);
            }
        } else {
            i += 1;
        }
    }
    0
}

/// A master playlist's levels as a download tiers them, in order: one that gives no size is as tall as its name says.
pub fn levels(text: &str, base: &str) -> Vec<Level> {
    let mut all = hls::levels(text, base);
    for level in all.iter_mut().filter(|l| l.height == 0) {
        let tall = height_in_name(&format!("{} {}", level.info, level.url));
        if tall > 0 {
            level.height = quality::nominal(level.width, tall);
        }
    }
    all
}

/// The level a download takes at most `height` ([`quality::pick_level`]); with no heights given at all, by weight:
/// the lightest that isn't the very lightest under 1080, the heaviest under 4K at 1080, the heaviest for Best
/// (`i64::MAX`).
pub fn pick(all: Vec<Level>, height: i64) -> Option<Level> {
    if !all.is_empty() && all.iter().all(|l| l.height == 0) {
        let mut by_weight = all;
        by_weight.sort_by_key(Level::rate);
        let n = by_weight.len();
        let i = if height == i64::MAX {
            n - 1
        } else if height >= 1080 {
            n.saturating_sub(2)
        } else {
            1.min(n - 1)
        };
        return by_weight.into_iter().nth(i);
    }
    let heights: Vec<i64> = all.iter().map(|l| l.height).collect();
    quality::pick_level(&heights, height).and_then(|i| all.into_iter().nth(i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hls::{join, written};

    const MASTER: &str = "#EXTM3U
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",LANGUAGE=\"fr\",DEFAULT=YES,URI=\"fr.m3u8\"
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",LANGUAGE=\"en\",URI=\"en.m3u8\"
#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1280x720,CODECS=\"avc1.64001f,mp4a.40.2\",AUDIO=\"aud\"
720/index.m3u8
#EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=5000000,BANDWIDTH=8940800,RESOLUTION=3840x1600,FRAME-RATE=23.976,AUDIO=\"aud\"
2160/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=4000000,RESOLUTION=1920x800
1080/index.m3u8";

    #[test]
    fn levels_named_and_picked() {
        let all = levels(MASTER, "");
        assert_eq!(all.iter().map(|l| l.height).collect::<Vec<_>>(), [720, 2160, 1080]);
        assert_eq!((all[1].rate(), all[1].average.is_some(), all[0].average.is_some()), (5_000_000, true, false));
        assert_eq!(all[1].name(), "4K");
        assert_eq!(pick(levels(MASTER, ""), 1080).map(|l| l.url), Some("1080/index.m3u8".into()));
        assert_eq!(pick(levels(MASTER, ""), 480).map(|l| l.url), Some("720/index.m3u8".into()));
        // (no sizes at all: by weight)
        let bare = "#EXT-X-STREAM-INF:BANDWIDTH=1\na\n#EXT-X-STREAM-INF:BANDWIDTH=3\nc\n#EXT-X-STREAM-INF:BANDWIDTH=2\nb";
        assert_eq!(pick(levels(bare, ""), 720).map(|l| l.url), Some("b".into()));
        assert_eq!(pick(levels(bare, ""), 1080).map(|l| l.url), Some("b".into()));
        assert_eq!(pick(levels(bare, ""), i64::MAX).map(|l| l.url), Some("c".into()));
        // (a height in the name)
        assert_eq!(levels("#EXT-X-STREAM-INF:BANDWIDTH=1\nvideo_1080p.m3u8", "")[0].height, 1080);
    }

    #[test]
    fn pieces_read() {
        let level = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4.0,\nseg0.m4s\n#EXTINF:5.5,\nseg1.m4s\n#EXTINF:2,\nseg2.m4s\n#EXT-X-ENDLIST";
        let read = written(level);
        assert_eq!(read.pieces.iter().map(|p| p.url.as_str()).collect::<Vec<_>>(), ["seg0.m4s", "seg1.m4s", "seg2.m4s"]);
        assert_eq!(read.init.as_deref(), Some("init.mp4"));
        assert_eq!(read.pieces.iter().map(|p| p.seconds).collect::<Vec<_>>(), [4., 5.5, 2.]);
        assert_eq!(read.length(), 11.5);
        assert_eq!(read.run(2), 9.5);
        assert!(!read.locked);
        assert!(written("#EXT-X-KEY:METHOD=AES-128,URI=\"k\"").locked);
        assert!(!written("#EXT-X-KEY:METHOD=NONE").locked);
        assert_eq!(join("https://cdn.x/a/b/master.m3u8", "../c/l.m3u8"), "https://cdn.x/a/c/l.m3u8");
        assert_eq!(join("https://cdn.x/a/master.m3u8", "https://o.y/p"), "https://o.y/p");
    }

    #[test]
    fn a_piece_without_its_length_is_left_out() {
        let level = "#EXTINF:4,\na.ts\nstray.ts\n#EXTINF:2,\nb.ts";
        assert_eq!(written(level).pieces.len(), 2);
    }

    #[test]
    fn the_player_and_downloads_read_one_master_alike() {
        let base = "https://cdn.x/films/1/master.m3u8?t=a";
        let player = hls::levels(MASTER, base);
        let downloads = levels(MASTER, base);
        assert_eq!(player, downloads);
        let heights: Vec<i64> = hls::ladder(&player).iter().map(|r| r.height).collect();
        assert_eq!(heights, [720, 1080, 2160]);
        let taken = pick(downloads, 1080).expect("a level");
        assert_eq!(taken.url, "https://cdn.x/films/1/1080/index.m3u8");
        assert!(player.contains(&taken));
    }
}
