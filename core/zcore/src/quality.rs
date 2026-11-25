//! Picture quality, for streaming and downloads alike: what a level is called, how tall an account may watch, and
//! which level or copy a setting picks.

/// The tallest a Movies & TV picture plays or downloads at for an account without 2K and 4K (the owner's switch).
pub const STANDARD_MOST: i64 = 1080;

/// Where Downloads' quality starts before the profile picks one: the server's default (profiles.py).
pub const DOWNLOAD_DEFAULT: &str = "high";

/// Settings' download qualities, as (value, label): Standard 720p, High 1080p, Best the tallest the account may have.
pub const DOWNLOAD_CHOICES: [(&str, &str); 3] = [("standard", "Standard"), ("high", "High"), ("best", "Best")];

/// Width cuts naming a level: at least this wide is this tall. 720p's sits at 1100, where the server cuts
/// them too: well clear of widescreen SD's 1024 and of 720p trimmed at the sides.
const CUTS: [(i64, i64); 5] = [(3500, 2160), (2400, 1440), (1800, 1080), (1100, 720), (800, 480)];

/// Settings' streaming qualities, as (value, label): 4K only for an account with it switched on (`plus`).
pub fn stream_choices(plus: bool) -> Vec<(&'static str, &'static str)> {
    [("auto", "Auto"), ("2160p", "4K"), ("1080p", "1080p"), ("720p", "720p"), ("480p", "480p")]
        .into_iter()
        .filter(|(value, _)| plus || *value != "2160p")
        .collect()
}

/// The streaming choice Settings shows for a `stored` setting. The player's Quality saves any level ("1440p",
/// "1080p HDR") and Settings offers fewer: the tallest offered at or under it, else the smallest; Auto for the rest.
pub fn shown_choice(stored: &str, plus: bool) -> &'static str {
    let wanted = height(stored);
    if wanted == 0 {
        return "auto";
    }
    let offered = stream_choices(plus);
    let fixed = || offered.iter().map(|(value, _)| *value).filter(|v| height(v) > 0);
    fixed().filter(|v| height(v) <= wanted).max_by_key(|v| height(v)).or_else(|| fixed().min_by_key(|v| height(v))).unwrap_or("auto")
}

/// A quality's height: "1080p" and "1080p HDR" are 1080; Auto, or anything else, 0.
pub fn height(quality: &str) -> i64 {
    let digits: String = quality.trim_start().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().unwrap_or(0)
}

/// Whether a quality asks for HDR ("2160p HDR").
pub fn wants_hdr(quality: &str) -> bool {
    quality.to_ascii_lowercase().contains("hdr")
}

/// The tallest level (by its name, [`nominal`]) a `source`'s picture plays or downloads at: Movies & TV stop at 1080p
/// for an account without 2K and 4K (`plus`); anything else has no limit.
pub fn most(source: &str, plus: bool) -> i64 {
    if source == "cinema" && !plus {
        STANDARD_MOST
    } else {
        i64::MAX
    }
}

/// The widest picture a level named at most `most` tall can be, for a player that caps by size. None: no limit.
pub fn widest(most: i64) -> Option<i64> {
    CUTS.iter().rev().find(|(_, tall)| *tall > most).map(|(wide, _)| wide - 1)
}

/// A level's height as it's named, by its width, so wide films' 3840x1600 and 3840x2080 are 4K and 1920x800 is 1080p.
/// With no width known (0), or under SD's, its height.
pub fn nominal(width: i64, height: i64) -> i64 {
    CUTS.iter().find(|(wide, _)| width >= *wide).map_or(height, |(_, tall)| *tall)
}

/// What a level is called, from its [`nominal`] height: "4K", "2K", "1080p", "720p"...
pub fn level_name(nominal: i64) -> String {
    match nominal {
        n if n >= 2160 => "4K".into(),
        n if n >= 1440 => "2K".into(),
        n => format!("{n}p"),
    }
}

/// A quality as it's shown: "2160p HDR" is "4K HDR", "1440p" "2K".
pub fn quality_name(quality: &str) -> String {
    match height(quality) {
        0 => quality.to_string(),
        h => quality.replacen(&format!("{h}p"), &level_name(h), 1),
    }
}

/// What a level's name stands for, to sort by ("4K" 2160, "720p" 720); 0 for anything else (an old "High").
pub fn name_rank(name: &str) -> i64 {
    match name {
        "4K" => 2160,
        "2K" => 1440,
        n => n.strip_suffix('p').and_then(|h| h.parse().ok()).unwrap_or(0),
    }
}

/// How tall a stream with levels `heights` (named, [`nominal`]) may play under `most`: `most`, or its smallest level
/// when it has none that low (something always plays).
pub fn ceiling(heights: &[i64], most: i64) -> i64 {
    if heights.iter().any(|h| *h <= most) {
        most
    } else {
        heights.iter().copied().min().unwrap_or(most)
    }
}

/// The level a fixed quality plays, or a download takes, among `heights` (named, [`nominal`]): the tallest at or under
/// `most`, else the smallest. The first of equals.
pub fn pick_level(heights: &[i64], most: i64) -> Option<usize> {
    let under = heights.iter().enumerate().filter(|(_, h)| (1..=most).contains(*h)).fold(None, |best, (i, h)| {
        match best {
            Some((_, b)) if b >= *h => best,
            _ => Some((i, *h)),
        }
    });
    under.map(|(i, _)| i).or_else(|| heights.iter().enumerate().min_by_key(|(_, h)| **h).map(|(i, _)| i))
}

/// The tallest level a download takes, by the setting ("standard", "high", "best") or a download's saved quality
/// ("720p", "1080p", "best"), never over what the account may have (`most`).
pub fn download_most(quality: &str, most: i64) -> i64 {
    match quality {
        "best" => most,
        "high" | "1080p" => most.min(1080),
        _ => most.min(720),
    }
}

/// A new download's saved quality and its label until its level is known, from the setting: ("720p", "Standard"),
/// ("1080p", "High") or ("best", "Best").
pub fn download_saved(setting: &str) -> (&'static str, &'static str) {
    match setting {
        "best" => ("best", "Best"),
        "high" => ("1080p", "High"),
        _ => ("720p", "Standard"),
    }
}

/// A copy of a film, as the choice below weighs it: its height (from its quality, "2160p") and whether it's HDR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Copy {
    pub height: i64,
    pub hdr: bool,
}

/// The copy for a `quality` setting: that height (HDR as the setting says, if it can), else the tallest under
/// it, else the smallest; Auto takes 1080p if there's one, else the tallest. The server sends only what the account
/// may play.
pub fn choose_copy_by_setting(copies: &[Copy], quality: &str) -> Option<usize> {
    let wanted = height(quality);
    let hdr = wants_hdr(quality);
    let mut order: Vec<usize> = (0..copies.len()).collect();
    order.sort_by_key(|i| (std::cmp::Reverse(copies[*i].height), copies[*i].hdr != hdr));
    let first = |keep: &dyn Fn(i64) -> bool| order.iter().copied().find(|i| keep(copies[*i].height));
    if wanted == 0 {
        return first(&|h| h == 1080).or_else(|| order.first().copied());
    }
    first(&|h| h == wanted).or_else(|| first(&|h| h < wanted)).or_else(|| order.last().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(height: i64, hdr: bool) -> Copy {
        Copy { height, hdr }
    }

    #[test]
    fn settings_offer_4k_to_plus_only() {
        assert!(stream_choices(true).iter().any(|c| c.0 == "2160p"));
        assert!(!stream_choices(false).iter().any(|c| c.0 == "2160p"));
        assert_eq!(shown_choice("1440p", true), "1080p");
        assert_eq!(shown_choice("2160p HDR", true), "2160p");
        assert_eq!(shown_choice("2160p", false), "1080p");
        assert_eq!(shown_choice("360p", false), "480p");
        assert_eq!(shown_choice("auto", false), "auto");
    }

    #[test]
    fn levels_are_named_by_width() {
        assert_eq!(nominal(3840, 1600), 2160);
        assert_eq!(nominal(2560, 1080), 1440);
        assert_eq!(nominal(1920, 800), 1080);
        assert_eq!(nominal(1280, 536), 720);
        assert_eq!(nominal(1152, 480), 720);
        assert_eq!(nominal(1024, 576), 480);
        assert_eq!(nominal(640, 360), 360);
        assert_eq!(nominal(0, 720), 720);
        assert_eq!(level_name(2160), "4K");
        assert_eq!(level_name(1440), "2K");
        assert_eq!(level_name(720), "720p");
        assert_eq!(quality_name("2160p HDR"), "4K HDR");
        assert_eq!(quality_name("auto"), "auto");
        assert_eq!(name_rank("2K"), 1440);
        assert_eq!(name_rank("High"), 0);
        assert_eq!(height(" 1080p HDR"), 1080);
    }

    #[test]
    fn the_account_cap() {
        assert_eq!(most("cinema", false), 1080);
        assert_eq!(most("cinema", true), i64::MAX);
        assert_eq!(most("youtube", false), i64::MAX);
        assert_eq!(widest(1080), Some(2399));
        assert_eq!(widest(720), Some(1799));
        assert_eq!(widest(i64::MAX), None);
        assert_eq!(ceiling(&[2160, 1080, 720], 1080), 1080);
        assert_eq!(ceiling(&[2160, 1440], 1080), 1440);
    }

    #[test]
    fn levels_picked() {
        let heights = [2160, 1080, 720, 480];
        assert_eq!(pick_level(&heights, 1440), Some(1));
        assert_eq!(pick_level(&heights, 360), Some(3));
        assert_eq!(pick_level(&heights, i64::MAX), Some(0));
        assert_eq!(pick_level(&[], 720), None);
        assert_eq!(download_most("best", 1080), 1080);
        assert_eq!(download_most("best", i64::MAX), i64::MAX);
        assert_eq!(download_most("high", i64::MAX), 1080);
        assert_eq!(download_most("720p", i64::MAX), 720);
        assert_eq!(download_saved("high"), ("1080p", "High"));
    }

    #[test]
    fn copy_for_a_setting() {
        let all = [copy(2160, true), copy(2160, false), copy(1080, false), copy(720, false)];
        assert_eq!(choose_copy_by_setting(&all, "auto"), Some(2));
        assert_eq!(choose_copy_by_setting(&all, "2160p"), Some(1));
        assert_eq!(choose_copy_by_setting(&all, "2160p HDR"), Some(0));
        assert_eq!(choose_copy_by_setting(&all, "1440p"), Some(2));
        assert_eq!(choose_copy_by_setting(&all, "480p"), Some(3));
        assert_eq!(choose_copy_by_setting(&all[..2], "auto"), Some(1));
        assert_eq!(choose_copy_by_setting(&[], "auto"), None);
    }
}
