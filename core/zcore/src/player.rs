//! Player rules every app applies the same way: which skip buttons a video gets, how an episode is named, and when
//! Next episode comes up and plays.

/// Seconds Next episode counts down from when it shows (Netflix's ~5).
pub const UP_NEXT: f64 = 5.;
/// With no end credits known, Next episode shows this long before the end.
pub const NO_CREDITS: f64 = 20.;
/// Videos this short (seconds) never offer Next episode: trailers, clips.
const SHORT: f64 = 300.;

/// A stretch of a video that can be skipped: an intro, a recap or the end credits.
/// `end: None` means it runs to the end of the video (only end credits do).
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub kind: String,
    pub start: f64,
    pub end: Option<f64>,
}

/// The skip segments worth offering in a video `total` seconds long.
/// - Segments starting in the last 5 s are dropped: too late to be worth a button.
/// - End credits ending within 3 s of the end run to the end.
/// - A segment that runs to the end is only kept when there's a next episode to go to (`has_next`).
pub fn fit_skip(found: Vec<Segment>, total: f64, has_next: bool) -> Vec<Segment> {
    found
        .into_iter()
        .filter(|s| s.start < total - 5.)
        .map(
            |s| {
                if s.kind == "credits" && s.end.is_none_or(|e| e >= total - 3.) {
                    Segment { end: None, ..s }
                } else {
                    s
                }
            },
        )
        .filter(|s| s.end.is_some() || has_next)
        .collect()
}

/// Where a video's skip segments come from, by its source. Whatever comes goes through [`fit_skip`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipsFrom {
    /// Movies & TV: TheIntroDB, asked from the device for the copy's length (`crate::intro`).
    IntroDb,
    /// Anime: sent with the stream.
    Stream,
    /// A YouTube drama's episode: the server listens for them, a new episode taking it about a minute, so a "pending"
    /// answer is asked again every [`SKIP_ASK_EVERY`] seconds, [`SKIP_ASKS`] times at most.
    Server,
}

/// Times a pending YouTube answer is asked for, all told.
pub const SKIP_ASKS: u32 = 6;
/// Seconds between those asks.
pub const SKIP_ASK_EVERY: f64 = 30.;

/// Where a `source`'s skip segments come from (`episode`: an episode is playing); None: it has none.
pub fn skips_from(source: &str, episode: bool) -> Option<SkipsFrom> {
    match source {
        "cinema" => Some(SkipsFrom::IntroDb),
        "anime" => Some(SkipsFrom::Stream),
        "youtube" if episode => Some(SkipsFrom::Server),
        _ => None,
    }
}

/// An episode as the player and Continue Watching name it: "S2 E1"; "E1" with no season; with no numbers at all,
/// its season's own name (anime specials, dramas' named parts).
pub fn episode_label(season: Option<i64>, number: Option<i64>, season_name: Option<&str>) -> String {
    let parts: Vec<String> =
        [season.map(|n| format!("S{n}")), number.map(|n| format!("E{n}"))].into_iter().flatten().collect();
    if parts.is_empty() {
        season_name.unwrap_or_default().to_string()
    } else {
        parts.join(" ")
    }
}

/// Where the end credits start: the latest verified credits that run to the end, else where one or two people say
/// they start (kind "end"). None: not known.
pub fn end_credits(segments: &[Segment]) -> Option<f64> {
    let latest =
        |kind: &str| segments.iter().filter(|s| s.kind == kind && s.end.is_none()).map(|s| s.start).reduce(f64::max);
    latest("credits").or_else(|| latest("end"))
}

/// The end of an episode, as Netflix shows it: from where its credits start (none known: its last 20 s) Next episode
/// and Watch credits come up and count down 5 s, then the next episode plays. With no credits known it counts to the
/// end instead, as what's left may still be the episode. One per episode playing.
#[derive(Clone, Debug, Default)]
pub struct Ending {
    shown_at: Option<f64>,
    held: bool,
    watching_credits: bool,
}

/// Next episode on screen: how far its countdown is (`count`, 0 to 1; 0 while it isn't counting) and the seconds
/// until it plays the next episode (`left`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Up {
    pub count: f64,
    pub left: f64,
}

impl Ending {
    /// Notes the new position `at` (of `total`, end credits from `credits`): where Next episode came up, or, back
    /// before that, that it's gone (and with it any hold).
    pub fn track(&mut self, at: f64, total: f64, credits: Option<f64>) {
        if at < credits.unwrap_or(total - NO_CREDITS) {
            self.shown_at = None;
            self.held = false;
        } else if self.shown_at.is_none() {
            self.shown_at = Some(at);
        }
    }

    /// Next episode as it stands at `at`, None while it isn't up. It doesn't count with `autoplay` off, once held,
    /// or at the very end; a count of 1 means play the next episode now.
    pub fn now(&self, at: f64, total: f64, credits: Option<f64>, has_next: bool, autoplay: bool) -> Option<Up> {
        if !has_next || total <= SHORT || self.watching_credits && at < total - 1. {
            return None;
        }
        let shown = self.shown_at?;
        let span = if credits.is_some() { UP_NEXT } else { (total - shown).max(1.) };
        if !autoplay || self.held || at >= total {
            return Some(Up { count: 0., left: span });
        }
        let count = ((at - shown) / span).clamp(0., 1.);
        Some(Up { count, left: (1. - count) * span })
    }

    /// Paused, or sought back, while Next episode is up: it stops counting, and the end of the video plays the next
    /// episode instead.
    pub fn hold(&mut self) {
        self.held = true;
    }

    /// Watch credits: Next episode put away until the last second of the video.
    pub fn watch_credits(&mut self) {
        self.watching_credits = true;
    }
}

/// A time as the players show it: "4:05", or "1:02:03" from an hour.
pub fn clock(seconds: f64) -> String {
    let n = seconds.max(0.) as u64;
    if n >= 3600 {
        format!("{}:{:02}:{:02}", n / 3600, n / 60 % 60, n % 60)
    } else {
        format!("{}:{:02}", n / 60, n % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(kind: &str, start: f64, end: Option<f64>) -> Segment {
        Segment { kind: kind.into(), start, end }
    }

    #[test]
    fn credits_near_the_end_run_to_the_end() {
        let fitted = fit_skip(vec![seg("credits", 1300., Some(1398.))], 1400., true);
        assert_eq!(fitted, vec![seg("credits", 1300., None)]);
    }

    #[test]
    fn open_ended_credits_need_a_next_episode() {
        assert!(fit_skip(vec![seg("credits", 1300., None)], 1400., false).is_empty());
        assert_eq!(fit_skip(vec![seg("intro", 30., Some(90.))], 1400., false), vec![seg("intro", 30., Some(90.))]);
    }

    #[test]
    fn segments_starting_in_the_last_seconds_are_dropped() {
        assert!(fit_skip(vec![seg("credits", 1397., None)], 1400., true).is_empty());
    }

    const LONG: f64 = 1400.;

    fn ending_at(at: f64, credits: Option<f64>) -> (Ending, Option<Up>) {
        let mut ending = Ending::default();
        ending.track(credits.unwrap_or(LONG - NO_CREDITS), LONG, credits);
        ending.track(at, LONG, credits);
        let up = ending.now(at, LONG, credits, true, true);
        (ending, up)
    }

    #[test]
    fn end_credits_prefer_verified() {
        let both = [seg("end", 1310., None), seg("credits", 1300., None), seg("credits", 1320., Some(1330.))];
        assert_eq!(end_credits(&both), Some(1300.));
        assert_eq!(end_credits(&[seg("end", 1310., None)]), Some(1310.));
        assert_eq!(end_credits(&[seg("intro", 30., Some(90.))]), None);
    }

    #[test]
    fn credits_known_count_five_seconds() {
        assert_eq!(ending_at(1299., Some(1300.)).1, None);
        assert_eq!(ending_at(1302., Some(1300.)).1, Some(Up { count: 0.4, left: 3. }));
        assert_eq!(ending_at(1310., Some(1300.)).1.map(|u| u.count), Some(1.));
    }

    #[test]
    fn no_credits_counts_to_the_end() {
        let (_, up) = ending_at(1390., None);
        assert_eq!(up, Some(Up { count: 0.5, left: 10. }));
    }

    #[test]
    fn held_stops_counting_until_back_before_the_start() {
        let (mut ending, _) = ending_at(1302., Some(1300.));
        ending.hold();
        assert_eq!(ending.now(1303., LONG, Some(1300.), true, true).map(|u| u.count), Some(0.));
        ending.track(1200., LONG, Some(1300.));
        ending.track(1301., LONG, Some(1300.));
        assert_eq!(ending.now(1303.5, LONG, Some(1300.), true, true).map(|u| u.count), Some(0.5));
    }

    #[test]
    fn not_up_without_next_short_videos_or_watching_credits() {
        let (mut ending, _) = ending_at(1302., Some(1300.));
        assert_eq!(ending.now(1302., LONG, Some(1300.), false, true), None);
        assert_eq!(ending.now(250., 290., None, true, true), None);
        assert_eq!(ending.now(1302., LONG, Some(1300.), true, false).map(|u| u.count), Some(0.));
        ending.watch_credits();
        assert_eq!(ending.now(1350., LONG, Some(1300.), true, true), None);
        assert!(ending.now(1399.5, LONG, Some(1300.), true, true).is_some());
    }

    #[test]
    fn where_skips_come_from() {
        assert_eq!(skips_from("cinema", false), Some(SkipsFrom::IntroDb));
        assert_eq!(skips_from("youtube", false), None);
        assert_eq!(skips_from("youtube", true), Some(SkipsFrom::Server));
        assert_eq!(skips_from("ary", true), None);
    }

    #[test]
    fn labels() {
        assert_eq!(episode_label(Some(2), Some(1), None), "S2 E1");
        assert_eq!(episode_label(None, Some(7), Some("Part 1")), "E7");
        assert_eq!(episode_label(None, None, Some("OVA")), "OVA");
        assert_eq!(episode_label(None, None, None), "");
    }

    #[test]
    fn clocks() {
        assert_eq!(clock(245.9), "4:05");
        assert_eq!(clock(3723.), "1:02:03");
        assert_eq!(clock(-3.), "0:00");
    }
}
