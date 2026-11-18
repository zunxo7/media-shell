//! HLS playlists, read one way for the player, the cam test, the subtitle sync and downloads: a master playlist's
//! levels and sounds, and a level's pieces.

use crate::quality::{self, nominal};

/// One level of a master playlist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Level {
    /// Its playlist's whole address.
    pub url: String,
    pub width: i64,
    /// Its height as it's named ([`nominal`]), so a wide film's 3840x1600 is 2160; 0 when it gives no size.
    pub height: i64,
    /// Bits a second at its peak (BANDWIDTH).
    pub bandwidth: u64,
    /// Bits a second on average, when it says (AVERAGE-BANDWIDTH).
    pub average: Option<u64>,
    pub frame_rate: Option<f64>,
    /// Its #EXT-X-STREAM-INF line, as written: what a download's own master keeps of it.
    pub info: String,
}

impl Level {
    /// Bits a second its pieces come to: its stated average, else its peak.
    pub fn rate(&self) -> u64 {
        self.average.unwrap_or(self.bandwidth)
    }

    /// What it's called: "1080p", "4K"; "" when it gives no size.
    pub fn name(&self) -> String {
        if self.height > 0 {
            quality::level_name(self.height)
        } else {
            String::new()
        }
    }

    /// The group its sound comes from when the sound is a stream of its own (AUDIO).
    pub fn audio(&self) -> Option<&str> {
        attribute(&self.info, "AUDIO")
    }
}

/// An attribute's value in a tag's line (`NAME=value` or `NAME="value"`), the name not part of a longer one.
pub fn attribute<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = line.split_once(':').map_or(line, |(_, rest)| rest);
    while !rest.is_empty() {
        let (key, after) = rest.split_once('=')?;
        let (value, next) = match after.strip_prefix('"') {
            Some(quoted) => {
                let (v, n) = quoted.split_once('"').unwrap_or((quoted, ""));
                (v, n.trim_start_matches(','))
            }
            None => after.split_once(',').unwrap_or((after, "")),
        };
        if key.trim() == name {
            return Some(value);
        }
        rest = next;
    }
    None
}

/// The levels of the master playlist `text`, found at `base`, in the playlist's order. A level without a bandwidth
/// is left out (the spec requires one, and no player can weigh it).
pub fn levels(text: &str, base: &str) -> Vec<Level> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("#EXT-X-STREAM-INF:"))
        .filter_map(|(i, line)| {
            let bandwidth = attribute(line, "BANDWIDTH")?.parse().ok()?;
            let (width, height) = attribute(line, "RESOLUTION")
                .and_then(|r| r.split_once('x'))
                .map(|(w, h)| (w.parse().unwrap_or(0), h.parse().unwrap_or(0)))
                .unwrap_or((0, 0));
            let uri = lines[i + 1..].iter().find(|u| !u.is_empty() && !u.starts_with('#'))?;
            Some(Level {
                url: join(base, uri),
                width,
                height: nominal(width, height),
                bandwidth,
                average: attribute(line, "AVERAGE-BANDWIDTH").and_then(|v| v.parse().ok()),
                frame_rate: attribute(line, "FRAME-RATE").and_then(|r| r.parse().ok()),
                info: (*line).into(),
            })
        })
        .collect()
}

/// A level as the player's Quality knows it: its height as named and its bandwidth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rung {
    pub height: i64,
    pub bandwidth: u64,
}

/// The distinct heights a stream offers, shortest first (each its lowest bandwidth), from 240 up.
pub fn ladder(levels: &[Level]) -> Vec<Rung> {
    let mut rungs: Vec<Rung> =
        levels.iter().filter(|l| l.height >= 240).map(|l| Rung { height: l.height, bandwidth: l.bandwidth }).collect();
    rungs.sort_unstable_by_key(|r| (r.height, r.bandwidth));
    rungs.dedup_by_key(|r| r.height);
    rungs
}

/// The level with the least bandwidth: every level carries the same sound, so it's what the sound is read from.
pub fn smallest(levels: &[Level]) -> Option<&Level> {
    levels.iter().min_by_key(|l| l.bandwidth)
}

/// The frame rate the stream's video runs at (its first level that says).
pub fn frame_rate(levels: &[Level]) -> Option<f64> {
    levels.iter().find_map(|l| l.frame_rate)
}

/// A sound of a master playlist that's a stream of its own (#EXT-X-MEDIA, TYPE=AUDIO, with a URI).
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    pub group: String,
    pub language: Option<String>,
    pub default: bool,
    /// Its playlist's whole address.
    pub url: String,
}

/// The sounds of the master playlist `text`, found at `base`, in the playlist's order.
pub fn sounds(text: &str, base: &str) -> Vec<Sound> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("#EXT-X-MEDIA:") && attribute(l, "TYPE") == Some("AUDIO"))
        .filter_map(|l| {
            Some(Sound {
                group: attribute(l, "GROUP-ID")?.into(),
                language: attribute(l, "LANGUAGE").map(String::from),
                default: attribute(l, "DEFAULT") == Some("YES"),
                url: join(base, attribute(l, "URI")?),
            })
        })
        .collect()
}

/// One piece of a level: where it starts in the film, how long it is, and its address.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub at: f64,
    pub seconds: f64,
    pub url: String,
}

/// A level's playlist: its init piece (fMP4), its pieces, and whether they can be fetched as they are.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    pub init: Option<String>,
    pub pieces: Vec<Piece>,
    /// Its pieces are encrypted (#EXT-X-KEY) or cut from one file (#EXT-X-BYTERANGE): they can't be joined as fetched.
    pub locked: bool,
}

impl Media {
    /// Its length in seconds: its pieces added up.
    pub fn length(&self) -> f64 {
        self.run(usize::MAX)
    }

    /// Seconds in its first `first` pieces.
    pub fn run(&self, first: usize) -> f64 {
        self.pieces.iter().take(first).map(|p| p.seconds).sum()
    }

    /// The pieces covering `seconds` from `from`.
    pub fn stretch(&self, from: f64, seconds: f64) -> Vec<&Piece> {
        self.pieces.iter().filter(|p| p.at + p.seconds > from && p.at < from + seconds).collect()
    }
}

/// The level playlist `text`, found at `base`.
pub fn media(text: &str, base: &str) -> Media {
    read(text, |uri: &str| join(base, uri))
}

/// The level playlist `text` with its addresses as written: its lengths and locks without making every address whole.
pub fn written(text: &str) -> Media {
    read(text, |uri: &str| uri.to_string())
}

/// A piece is an address after an #EXTINF; one without is no piece (the spec requires it), so every reader numbers
/// the pieces alike.
fn read(text: &str, address: impl Fn(&str) -> String) -> Media {
    let mut media = Media::default();
    let mut at = 0.;
    let mut seconds: Option<f64> = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with("#EXT-X-MAP:") {
            if media.init.is_none() {
                media.init = attribute(line, "URI").map(&address);
            }
        } else if let Some(info) = line.strip_prefix("#EXTINF:") {
            seconds = Some(info.split(',').next().and_then(|s| s.trim().parse().ok()).unwrap_or(0.));
        } else if line.starts_with("#EXT-X-KEY:") {
            media.locked |= attribute(line, "METHOD") != Some("NONE");
        } else if line.starts_with("#EXT-X-BYTERANGE") {
            media.locked = true;
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some(length) = seconds.take() {
                media.pieces.push(Piece { at, seconds: length, url: address(line) });
                at += length;
            }
        }
    }
    media
}

/// `reference` as a whole address, found in a playlist at `base` (RFC 3986's resolution); as written when `base`
/// isn't a whole address.
pub fn join(base: &str, reference: &str) -> String {
    url::Url::parse(base).and_then(|b| b.join(reference)).map_or_else(|_| reference.into(), String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "#EXTM3U\n\
        #EXT-X-STREAM-INF:BANDWIDTH=6121397,RESOLUTION=3840x1600,FRAME-RATE=24.000\n4k/index.m3u8\n\
        #EXT-X-STREAM-INF:BANDWIDTH=900000,RESOLUTION=1280x536\n/abs/720.m3u8\n\
        #EXT-X-STREAM-INF:BANDWIDTH=950000,RESOLUTION=1280x720\n720b.m3u8\n\
        #EXT-X-STREAM-INF:RESOLUTION=640x360\nnobandwidth.m3u8\n";

    #[test]
    fn master_levels_named_by_width_and_joined() {
        let found = levels(MASTER, "https://cdn.x/films/1/master.m3u8?token=a");
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].url, "https://cdn.x/films/1/4k/index.m3u8");
        assert_eq!((found[0].height, found[0].frame_rate), (2160, Some(24.)));
        assert_eq!(found[1].url, "https://cdn.x/abs/720.m3u8");
        let rungs = ladder(&found);
        assert_eq!(rungs, vec![Rung { height: 720, bandwidth: 900_000 }, Rung { height: 2160, bandwidth: 6_121_397 }]);
        assert_eq!(smallest(&found).map(|l| l.bandwidth), Some(900_000));
        assert_eq!(frame_rate(&found), Some(24.));
    }

    #[test]
    fn a_level_and_its_pieces() {
        let text = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:6.0,\np0.m4s\n#EXTINF:6.000,\np1.m4s?x=1\n#EXTINF:4,\n\
                    https://other/p2.m4s\n#EXT-X-ENDLIST\n";
        let m = media(text, "https://cdn.x/a/b/index.m3u8");
        assert_eq!(m.init.as_deref(), Some("https://cdn.x/a/b/init.mp4"));
        assert_eq!(m.pieces.len(), 3);
        assert_eq!(m.pieces[1], Piece { at: 6., seconds: 6., url: "https://cdn.x/a/b/p1.m4s?x=1".into() });
        assert_eq!(m.length(), 16.);
        assert_eq!(m.stretch(7., 3.).len(), 1);
        assert_eq!(m.stretch(5., 30.).len(), 3);
    }

    #[test]
    fn addresses_join_as_browsers_do() {
        let base = "https://h.x/a/b/c.m3u8?q=1";
        assert_eq!(join(base, "d.ts"), "https://h.x/a/b/d.ts");
        assert_eq!(join(base, "../d.ts"), "https://h.x/a/d.ts");
        assert_eq!(join(base, "./e/../d.ts?k=v"), "https://h.x/a/b/d.ts?k=v");
        assert_eq!(join(base, "/root.ts"), "https://h.x/root.ts");
        assert_eq!(join(base, "//cdn.y/z.ts"), "https://cdn.y/z.ts");
        assert_eq!(join(base, "http://o/z.ts"), "http://o/z.ts");
        assert_eq!(join("https://h.x", "z.ts"), "https://h.x/z.ts");
    }

    #[test]
    fn sounds_quoted_attributes_and_locks() {
        let master = "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",LANGUAGE=\"en\",DEFAULT=YES,URI=\"en/a.m3u8\"\n\
                      #EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"s\",URI=\"s.m3u8\"\n\
                      #EXT-X-STREAM-INF:BANDWIDTH=2,CODECS=\"avc1.64001f,mp4a.40.2\",RESOLUTION=1920x1080,AUDIO=\"aud\"\nv.m3u8";
        let found = sounds(master, "https://h.x/m/master.m3u8");
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].url.as_str(), found[0].default), ("https://h.x/m/en/a.m3u8", true));
        let level = &levels(master, "https://h.x/m/master.m3u8")[0];
        assert_eq!((level.height, level.audio(), level.rate()), (1080, Some("aud"), 2));
        assert!(written("#EXT-X-KEY:METHOD=AES-128,URI=\"k\"\n#EXTINF:1,\na.ts").locked);
        assert!(!written("#EXT-X-KEY:METHOD=NONE\n#EXTINF:1,\na.ts").locked);
        assert!(written("#EXTINF:1,\n#EXT-X-BYTERANGE:10@0\na.ts").locked);
        assert_eq!(written("#EXTINF:1,\na.ts\nstray.ts\n#EXTINF:2,\nb.ts").run(5), 3.);
    }
}
