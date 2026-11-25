//! Captions: WebVTT (or SRT, which reads the same here) into cues, what's on screen at a moment, and how big and in
//! what ink and box the profile has them (each app draws them itself).

/// One caption: from `start` to `end` (seconds), its words with the styling tags taken off (the apps draw their own).
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// "01:02:03.456", "02:03,456" or "3.5" in seconds; None when it isn't a time (the cue is skipped, not put at 0).
fn stamp(value: &str) -> Option<f64> {
    value.split(':').try_fold(0., |total, part| Some(total * 60. + part.replace(',', ".").parse::<f64>().ok()?))
}

/// The cues in `text`, by start. A cue with a time that can't be read, no words, or that doesn't end after it starts
/// is left out.
pub fn parse(text: &str) -> Vec<Cue> {
    let text = text.replace('\r', "");
    let mut cues: Vec<Cue> = text
        .split("\n\n")
        .filter_map(|block| {
            let mut lines = block.lines();
            let (a, b) = lines.find_map(|line| line.split_once("-->"))?;
            let start = stamp(a.trim())?;
            let end = stamp(b.split_whitespace().next()?)?;
            let mut inside = false;
            let words: String = lines
                .collect::<Vec<_>>()
                .join("\n")
                .chars()
                .filter(|c| match c {
                    '<' => {
                        inside = true;
                        false
                    }
                    '>' if inside => {
                        inside = false;
                        false
                    }
                    _ => !inside,
                })
                .collect::<String>()
                .trim()
                .to_string();
            (!words.is_empty() && start.is_finite() && end.is_finite() && end > start).then_some(Cue {
                start,
                end,
                text: words,
            })
        })
        .collect();
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    cues
}

/// What's on screen at `time`: the cues that have started and not ended (up to three overlapping), one per line.
pub fn at(cues: &[Cue], time: f64) -> String {
    let end = cues.partition_point(|cue| cue.start <= time);
    cues[end.saturating_sub(3)..end]
        .iter()
        .filter(|cue| time < cue.end)
        .map(|cue| cue.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The profile's caption sizes, as (value, label).
pub const SIZES: [(&str, &str); 3] = [("small", "Small"), ("medium", "Medium"), ("large", "Large")];

/// The caption inks, as (value, label, colour as 0xAARRGGBB).
pub const INKS: [(&str, &str, u32); 5] = [
    ("white", "White", 0xFFFF_FFFF),
    ("yellow", "Yellow", 0xFFFF_E066),
    ("cyan", "Cyan", 0xFF6F_E3F2),
    ("green", "Green", 0xFF7C_E38B),
    ("magenta", "Magenta", 0xFFF2_8AE6),
];

/// The box behind captions, as (value, label, how dark, 0 to 1): None draws an edge round the letters instead.
pub const SHADES: [(&str, &str, f64); 5] =
    [("none", "None", 0.), ("light", "Light", 0.35), ("medium", "Medium", 0.6), ("dark", "Dark", 0.85), ("solid", "Solid", 1.)];

/// A box lighter than this isn't drawn: a thin dark edge round the letters instead, so they read over anything.
pub const EDGED_BELOW: f64 = 0.15;

/// The captions' letter size: the design's caption size (`base`) at the profile's `setting`, grown on a big player
/// (`screen`: its short side, in the same units as `base`) up to 1.8 times.
pub fn size(base: f64, setting: &str, screen: f64) -> f64 {
    let scale = match setting {
        "small" => 0.8,
        "large" => 1.3,
        _ => 1.,
    };
    base * scale * (screen / 400.).clamp(1., 1.8)
}

/// An ink's colour (0xAARRGGBB); white for any other name.
pub fn ink(name: &str) -> u32 {
    INKS.iter().find(|(value, ..)| *value == name).map_or(INKS[0].2, |(_, _, colour)| *colour)
}

/// Whether captions on a box this dark (`background`, 0 to 1) are edged instead ([`EDGED_BELOW`]).
pub fn edged(background: f64) -> bool {
    background < EDGED_BELOW
}

/// How thick the edge round the letters is, for a letter `size`.
pub fn edge(size: f64) -> f64 {
    (size / 14.).max(1.2)
}

/// The shade Settings shows for a saved `background` (any number from 0 to 1): None below [`EDGED_BELOW`], as the
/// captions draw it, then the one it's nearest.
pub fn shade(background: f64) -> &'static str {
    match background {
        b if b < EDGED_BELOW => "none",
        b if b < 0.47 => "light",
        b if b < 0.72 => "medium",
        b if b < 0.95 => "dark",
        _ => "solid",
    }
}

/// How dark a shade is; Medium's for any other name.
pub fn shade_value(name: &str) -> f64 {
    SHADES.iter().find(|(value, ..)| *value == name).map_or(0.6, |(_, _, dark)| *dark)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VTT: &str = "WEBVTT\r\n\r\n2\r\n00:00:05.000 --> 00:00:07.500 align:start\r\n<i>Second</i> line\r\n\r\n\
                       1\r\n00:00:01,000 --> 00:00:04,000\r\nFirst\r\n\r\nbad --> 00:00:09.000\r\nskipped\r\n\r\n\
                       00:00:06.000 --> 00:00:06.000\r\nnever ends after it starts\r\n";

    #[test]
    fn parses_vtt_and_srt_times_tags_off_by_start() {
        let cues = parse(VTT);
        assert_eq!(
            cues,
            vec![
                Cue { start: 1., end: 4., text: "First".into() },
                Cue { start: 5., end: 7.5, text: "Second line".into() },
            ]
        );
    }

    #[test]
    fn looks() {
        assert_eq!(size(20., "large", 300.), 26.);
        assert_eq!(size(20., "small", 1000.), 20. * 0.8 * 1.8);
        assert_eq!(ink("yellow"), 0xFFFF_E066);
        assert_eq!(ink("purple"), 0xFFFF_FFFF);
        assert!(edged(0.1) && !edged(0.35));
        assert_eq!(shade(0.0), "none");
        assert_eq!(shade(0.5), "medium");
        assert_eq!(shade(0.9), "dark");
        assert_eq!(shade(0.97), "solid");
        assert_eq!(shade_value("light"), 0.35);
    }

    #[test]
    fn what_is_on_screen() {
        let cues = parse(VTT);
        assert_eq!(at(&cues, 0.5), "");
        assert_eq!(at(&cues, 2.), "First");
        assert_eq!(at(&cues, 4.5), "");
        assert_eq!(at(&cues, 6.), "Second line");
        let overlapping = parse("00:00:01.000 --> 00:00:09.000\nA\n\n00:00:02.000 --> 00:00:05.000\nB\n");
        assert_eq!(at(&overlapping, 3.), "A\nB");
    }
}
