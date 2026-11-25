//! Finding what to play: which source a title is tried from. The apps do the asking; these are the decisions.

/// Seconds a stream resolved on this device is used for before it's asked for again.
pub const STREAM_FRESH: f64 = 10. * 60.;
/// Seconds a title's TorBox copies are kept before they're asked for again.
pub const DEBRID_KEPT: f64 = 30. * 60.;

/// A place a stream is asked for, in the order a [`plan`] lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The anime server's stream for the episode.
    Anime,
    /// YouTube's, straight from YouTube, else through the server.
    YouTube,
    /// The TorBox copy, for the profile's quality (2K and 4K only for the accounts with them).
    Debrid,
    /// The stream the title page names (`hls`: an HLS playlist, whose captions the server can find).
    Page { hls: bool },
}

/// What's known of a title when Play is pressed.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ask<'a> {
    pub source: &'a str,
    pub kind: &'a str,
    /// An episode is playing, not a film.
    pub episode: bool,
    /// It has an IMDb id.
    pub imdb: bool,
    /// The stream its page names, if any.
    pub stream: Option<&'a str>,
}

/// Where to look for a title's stream, in order: each step's own failure moves on to the next. Empty: nothing of it
/// plays ("This one isn't playable yet.").
pub fn plan(ask: &Ask) -> Vec<Step> {
    match ask.source {
        "anime" if ask.episode => vec![Step::Anime],
        "youtube" if ask.episode => vec![Step::YouTube],
        "cinema" if ask.imdb => vec![Step::Debrid],
        _ => ask.stream.map(|s| vec![Step::Page { hls: is_hls(s) }]).unwrap_or_default(),
    }
}

/// Whether a stream's address is an HLS playlist.
pub fn is_hls(url: &str) -> bool {
    url.to_ascii_lowercase().contains("m3u8")
}

/// The languages from abroad, by the origin the server gives a title: (origin, ISO 639-1 code).
const ORIGINS: [(&str, &str); 7] = [
    ("korean", "ko"),
    ("turkish", "tr"),
    ("chinese", "zh"),
    ("japanese", "ja"),
    ("thai", "th"),
    ("spanish", "es"),
    ("indian", "hi"),
];

/// Every name a player may know a language's track by, ISO 639-2 first.
const NAMES: [(&str, &str); 8] = [
    ("ko", "kor,ko"),
    ("tr", "tur,tr"),
    ("zh", "chi,zho,zh,cmn"),
    ("ja", "jpn,ja"),
    ("th", "tha,th"),
    ("es", "spa,es"),
    ("hi", "hin,hi"),
    ("en", "eng,en"),
];

/// The sound a title plays in, best first (ISO 639-1 codes): a drama from abroad (`origin`) in its own language
/// unless the profile wants the dub (`dub`); anything else in English, as some copies carry another language first.
pub fn languages(origin: Option<&str>, dub: bool) -> Vec<&'static str> {
    match origin.and_then(|o| ORIGINS.iter().find(|(name, _)| *name == o)) {
        Some((_, own)) if !dub => vec![own, "en"],
        _ => vec!["en"],
    }
}

/// [`languages`] as a list of every name each goes by ("kor,ko,eng,en"), for players that match tracks by any of them
/// (mpv's alang).
pub fn track_names(languages: &[&str]) -> String {
    languages
        .iter()
        .map(|code| NAMES.iter().find(|(c, _)| c == code).map_or(*code, |(_, names)| names))
        .collect::<Vec<_>>()
        .join(",")
}

/// How a call failed, as an app tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The server or the source couldn't be reached: no connection, a timeout.
    Offline,
    /// The server answered with an error (its HTTP status). It reports its own failures.
    Answered(u16),
    /// Anything else: an answer that couldn't be read, the player failing on a stream.
    App,
}

/// Whether a failure goes to the error reports: only the app's own. Being offline, the server's answers (it reports
/// its own) and titles with nothing to play aren't bugs in the app.
pub fn worth_reporting(failure: Failure) -> bool {
    failure == Failure::App
}

#[cfg(test)]
mod tests {
    use super::*;

    fn film() -> Ask<'static> {
        Ask { source: "cinema", kind: "movie", imdb: true, ..Ask::default() }
    }

    #[test]
    fn plans() {
        assert_eq!(plan(&Ask { source: "anime", episode: true, ..Ask::default() }), vec![Step::Anime]);
        assert_eq!(plan(&Ask { source: "youtube", episode: true, ..Ask::default() }), vec![Step::YouTube]);
        assert_eq!(plan(&Ask { source: "ary", stream: Some("https://a/b.M3U8"), ..Ask::default() }), vec![Step::Page { hls: true }]);
        assert_eq!(plan(&Ask { source: "ary", stream: Some("https://a/b.mp4"), ..Ask::default() }), vec![Step::Page { hls: false }]);
        assert!(plan(&Ask { source: "ary", ..Ask::default() }).is_empty());
        assert_eq!(plan(&film()), vec![Step::Debrid]);
        let episode = Ask { episode: true, kind: "tv", ..film() };
        assert_eq!(plan(&episode), vec![Step::Debrid]);
    }

    #[test]
    fn no_imdb_no_cinema_plan() {
        let no_imdb = Ask { imdb: false, ..film() };
        assert!(plan(&no_imdb).is_empty());
    }

    #[test]
    fn sound() {
        assert_eq!(languages(Some("korean"), false), vec!["ko", "en"]);
        assert_eq!(languages(Some("korean"), true), vec!["en"]);
        assert_eq!(languages(None, false), vec!["en"]);
        assert_eq!(track_names(&languages(Some("chinese"), false)), "chi,zho,zh,cmn,eng,en");
    }

    #[test]
    fn reports() {
        assert!(worth_reporting(Failure::App));
        assert!(!worth_reporting(Failure::Offline));
        assert!(!worth_reporting(Failure::Answered(500)));
    }
}
