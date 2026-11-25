//! Anime art with writing on it. An anime's backdrop is often a poster with its Japanese name across it, and some of
//! its logos are in Japanese. The server reads a picture's writing once (Tesseract in English and Japanese,
//! `/api/arttext`) and keeps the answer; the apps ask about an anime's art only, and draw what this module says.

use serde_json::Value;

/// What writing the server read on a picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Writing {
    /// None worth speaking of.
    Clean,
    /// The title's own name: its logo over it would say it twice.
    Title,
    /// Other words.
    Other,
    /// Japanese, Chinese or Korean.
    Asian,
}

impl Writing {
    /// The number the answer is kept as.
    pub fn code(self) -> i64 {
        match self {
            Writing::Clean => 0,
            Writing::Title => 1,
            Writing::Other => 2,
            Writing::Asian => 3,
        }
    }

    /// The writing a kept number stands for.
    pub fn from_code(code: i64) -> Writing {
        match code {
            1 => Writing::Title,
            2 => Writing::Other,
            3 => Writing::Asian,
            _ => Writing::Clean,
        }
    }
}

/// The kept answer for [`writing`] (see [`Writing::code`]).
pub fn writing_code(answer: &str, title: &str) -> Option<i64> {
    writing(answer, title).map(Writing::code)
}

/// The kept answer for [`latin`]: 1 for a logo the viewers read, 0 for one they do not.
pub fn latin_code(answer: Option<&str>, refused: bool) -> Option<i64> {
    latin(answer, refused).map(i64::from)
}

/// The server's reading of a picture (`url`, as the app loads it).
pub fn path(url: &str) -> String {
    format!("/api/arttext?u={}", crate::sections::encode(url))
}

/// Whether to ask about a card's art at all: an anime's, unless its art is TMDB's (no writing on it).
pub fn worth_asking(source: Option<&str>, textless: bool) -> bool {
    source == Some("anime") && !textless
}

/// The writing in the server's answer about a picture of `title`; None when the answer can't be read.
pub fn writing(answer: &str, title: &str) -> Option<Writing> {
    let o: Value = serde_json::from_str(answer).ok()?;
    let text = o.get("text").and_then(Value::as_str).unwrap_or_default();
    Some(if o.get("cjk").and_then(Value::as_bool).unwrap_or(false) {
        Writing::Asian
    } else if mentions(text, title) {
        Writing::Title
    } else if text.chars().filter(char::is_ascii_alphabetic).count() >= 3 {
        Writing::Other
    } else {
        Writing::Clean
    })
}

/// Whether a logo is in a script the apps' viewers read: not when the server found Japanese, Chinese or Korean in
/// it. Stylised lettering often can't be read at all, and that's no reason to hide a logo; nor is a picture the
/// server wouldn't read (`refused`: it answered 400).
pub fn latin(answer: Option<&str>, refused: bool) -> Option<bool> {
    if refused {
        return Some(true);
    }
    let o: Value = serde_json::from_str(answer?).ok()?;
    Some(!o.get("cjk").and_then(Value::as_bool).unwrap_or(false))
}

/// The backdrop a landscape card shows: our own still of the show (`still`, a `/still/` picture) in place of one
/// with Asian writing on it. The writing on what it shows comes back with it.
pub fn card_cover<'a>(cover: &'a str, still: Option<&'a str>, writing: Option<Writing>) -> (&'a str, Option<Writing>) {
    match still.filter(|s| s.contains("/still/")) {
        Some(still) if writing == Some(Writing::Asian) => (still, Some(Writing::Clean)),
        _ => (cover, writing),
    }
}

/// Whether a card or title page shows a title's logo (it has one): not an anime logo in Asian script (`checked`:
/// [`worth_asking`] said its art is worth asking about; `latin`: what's known of its logo), nor over a backdrop that
/// already has the name (or Asian writing) on it (`writing`: what the backdrop it's over has on it; None for a page
/// whose backdrop isn't under the logo).
pub fn logo_shown(checked: bool, latin: Option<bool>, writing: Option<Writing>) -> bool {
    (!checked || latin != Some(false)) && !matches!(writing, Some(Writing::Title) | Some(Writing::Asian))
}

/// Whether a card with no logo shown writes the title's name over its backdrop: not when the backdrop already has
/// it, except for Pakistan's dramas (`drama`), which always carry their name.
pub fn name_shown(writing: Option<Writing>, drama: bool) -> bool {
    drama || writing != Some(Writing::Title)
}

/// What a landscape card needs of a title to choose its art.
#[derive(Clone, Copy, Debug, Default)]
pub struct CardFacts<'a> {
    pub source: Option<&'a str>,
    pub textless: bool,
    pub cover: Option<&'a str>,
    pub cover_big: Option<&'a str>,
    pub poster: Option<&'a str>,
    pub logo: Option<&'a str>,
    pub title: &'a str,
}

/// What an app knows of the writing on pictures (None: not known yet, and asked for). Both are asked about only
/// when [`worth_asking`] says so; the pictures are named as the card has them (the app adds its host).
pub trait Looks {
    /// The writing on a backdrop of `title`.
    fn writing(&self, picture: &str, title: &str) -> Option<Writing>;
    /// Whether a logo is in a script the viewers read.
    fn latin(&self, logo: &str) -> Option<bool>;
}

/// How a landscape card shows a title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardArt {
    /// The backdrop (None: the title has no picture).
    pub cover: Option<String>,
    /// Its logo, when one goes over the backdrop.
    pub logo: Option<String>,
    /// Its name goes over the backdrop (there's no logo, and the backdrop doesn't have the name already).
    pub name: bool,
}

/// A landscape card's art, whole: the backdrop (our own still in place of one with Asian writing), the logo or else
/// the name over it, by what [`Looks`] knows. Everything but an anime's art is never held up by any of it.
pub fn card_art(f: &CardFacts, looks: &dyn Looks) -> CardArt {
    let cover = crate::pictures::landscape(f.cover, f.cover_big, f.poster);
    let checked = worth_asking(f.source, f.textless);
    let read = match cover {
        Some(c) if checked => looks.writing(c, f.title),
        _ => Some(Writing::Clean),
    };
    let shown = cover.map(|c| card_cover(c, f.cover_big, read));
    let writing = shown.map_or(read, |(_, w)| w);
    let logo = f
        .logo
        .filter(|l| !l.is_empty() && logo_shown(checked, if checked { looks.latin(l) } else { None }, writing))
        .map(str::to_string);
    let name = logo.is_none() && name_shown(writing, crate::pictures::drama(f.source));
    CardArt { cover: shown.map(|(c, _)| c.to_string()).or_else(|| cover.map(str::to_string)), logo, name }
}

/// Whether the words read include the title: half or more of its words (of three letters and up).
fn mentions(text: &str, title: &str) -> bool {
    let norm = |s: &str| s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>();
    let read = norm(text);
    let joined: String = read.split_whitespace().collect();
    let all: Vec<String> = norm(title).split_whitespace().map(str::to_string).collect();
    let long: Vec<&String> = all.iter().filter(|w| w.chars().count() >= 3).collect();
    let words: Vec<&String> = if long.is_empty() { all.iter().collect() } else { long };
    if words.is_empty() {
        return false;
    }
    let found = words
        .iter()
        .filter(|w| read.split_whitespace().any(|r| r == w.as_str()) || (w.chars().count() >= 4 && joined.contains(w.as_str())))
        .count();
    found * 2 >= words.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_servers_answer() {
        assert_eq!(writing(r#"{"text": "ジョジョ", "cjk": true}"#, "JoJo"), Some(Writing::Asian));
        assert_eq!(writing(r#"{"text": "ATTACK ON TITAN", "cjk": false}"#, "Attack on Titan"), Some(Writing::Title));
        assert_eq!(writing(r#"{"text": "SEASON FINALE"}"#, "Attack on Titan"), Some(Writing::Other));
        assert_eq!(writing(r#"{"text": "a1"}"#, "Attack on Titan"), Some(Writing::Clean));
        assert_eq!(writing("oops", "x"), None);
        assert_eq!(latin(Some(r#"{"cjk": true}"#), false), Some(false));
        assert_eq!(latin(Some(r#"{"text": ""}"#), false), Some(true));
        assert_eq!(latin(None, true), Some(true));
        assert_eq!(latin(None, false), None);
        assert_eq!(writing_code(r#"{"cjk": true}"#, "x"), Some(3));
        assert_eq!(Writing::from_code(Writing::Other.code()), Writing::Other);
        assert_eq!((latin_code(Some(r#"{"cjk": true}"#), false), latin_code(None, true)), (Some(0), Some(1)));
        assert_eq!(path("https://x/a b.jpg"), "/api/arttext?u=https%3A%2F%2Fx%2Fa+b.jpg");
    }

    #[test]
    fn a_cards_art_follows_what_is_known() {
        struct Known(Option<Writing>, Option<bool>);
        impl Looks for Known {
            fn writing(&self, _: &str, _: &str) -> Option<Writing> {
                self.0
            }
            fn latin(&self, _: &str) -> Option<bool> {
                self.1
            }
        }
        let anime = CardFacts { source: Some("anime"), cover: Some("c.jpg"), cover_big: Some("/still/1.jpg"), logo: Some("l.png"), title: "Show", ..Default::default() };
        // (Asian writing on the backdrop: our own still instead, and the logo stays)
        let a = card_art(&anime, &Known(Some(Writing::Asian), Some(true)));
        assert_eq!((a.cover.as_deref(), a.logo.as_deref(), a.name), (Some("/still/1.jpg"), Some("l.png"), false));
        // (a logo in Japanese: the name instead)
        let a = card_art(&anime, &Known(Some(Writing::Clean), Some(false)));
        assert_eq!((a.logo, a.name), (None, true));
        // (the backdrop has the name already: neither)
        let a = card_art(&anime, &Known(Some(Writing::Title), None));
        assert_eq!((a.logo, a.name), (None, false));
        // (not an anime's: never asked, never held up)
        let film = CardFacts { source: Some("cinema"), ..anime };
        let a = card_art(&film, &Known(Some(Writing::Asian), Some(false)));
        assert_eq!((a.cover.as_deref(), a.logo.as_deref()), (Some("c.jpg"), Some("l.png")));
        assert_eq!(card_art(&CardFacts { source: Some("cinema"), title: "X", ..Default::default() }, &Known(None, None)), CardArt { cover: None, logo: None, name: true });
    }

    #[test]
    fn what_a_card_shows() {
        assert!(worth_asking(Some("anime"), false) && !worth_asking(Some("anime"), true) && !worth_asking(None, false));
        assert_eq!(card_cover("poster", Some("/still/1.jpg"), Some(Writing::Asian)), ("/still/1.jpg", Some(Writing::Clean)));
        assert_eq!(card_cover("poster", Some("other.jpg"), Some(Writing::Asian)), ("poster", Some(Writing::Asian)));
        assert_eq!(card_cover("poster", Some("/still/1.jpg"), Some(Writing::Title)), ("poster", Some(Writing::Title)));
        assert!(logo_shown(true, Some(true), Some(Writing::Clean)));
        assert!(logo_shown(true, None, None) && logo_shown(false, Some(false), None));
        assert!(!logo_shown(true, Some(false), None) && !logo_shown(true, Some(true), Some(Writing::Title)));
        assert!(!name_shown(Some(Writing::Title), false) && name_shown(Some(Writing::Title), true) && name_shown(None, false));
    }
}
