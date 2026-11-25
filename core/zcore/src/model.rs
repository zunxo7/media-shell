//! What the server sends about titles, read the same way by every app: cards (`Item`), title pages (`Title`) and their
//! episodes and seasons. Reading is lenient, as the server's sources are: a missing or empty field is None, a number
//! may come as a string, and anything unreadable falls back to its default rather than failing the whole page.

use serde_json::Value;

/// A title as a card shows it: Home's rows, the sections, search.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub poster: Option<String>,
    pub cover: Option<String>,
    pub cover_big: Option<String>,
    pub fallback: Option<String>,
    pub poster_hd: Option<String>,
    pub logo: Option<String>,
    /// cinema, anime, youtube, ary (none: ARY).
    pub source: Option<String>,
    /// "movie", or what a show is.
    pub kind: Option<String>,
    pub episodes: Option<i64>,
    pub year: Option<String>,
    pub genres: Vec<String>,
    pub description: Option<String>,
    pub age: Option<String>,
    pub channel: Option<String>,
    pub minutes: Option<i64>,
    /// IMDb's score, "8.2".
    pub rating: Option<String>,
    /// Continue watching / New episodes: the saved spot (or episode) to carry on from. Set by the app, not sent.
    pub resume: Option<String>,
    /// The line under the name when the row words it ("E3 · 12m left"). Set by the app, not sent.
    pub sub: Option<String>,
    /// Its poster is art with no writing on it (TMDB's): the card lays the logo over it.
    pub poster_clean: bool,
    /// Its cover is TMDB's backdrop with no writing on it, its logo TMDB's English one.
    pub textless: bool,
    /// A drama from abroad ("korean"): it belongs in Dramas.
    pub origin: Option<String>,
    /// The day what's new about it came (unix seconds), and what it was: "episode", "season" or "added".
    pub new_at: Option<i64>,
    pub new_kind: Option<String>,
    /// Not out yet: when it can be played (unix seconds). None: it plays now.
    pub available_at: Option<i64>,
}

impl Item {
    pub fn is_movie(&self) -> bool {
        self.kind.as_deref() == Some("movie")
    }

    /// Its portrait art has no writing on it (TMDB's, or ARY's big portrait art): a card writes its name over it.
    pub fn clean_art(&self) -> bool {
        clean_art(self.poster_clean, self.source.as_deref(), self.poster_hd.as_deref())
    }

    /// Clean art, and a logo to carry over it.
    pub fn own_art(&self) -> bool {
        own_art(self.clean_art(), self.logo.as_deref())
    }

    /// What a card calls it: "Anime", "Movie", "TV Show" or "Drama".
    pub fn what(&self) -> &'static str {
        what(self.source.as_deref(), self.kind.as_deref())
    }

    /// A card's facts: "2023  •  1h 45m" for a film, "2026  •  TV Show" for anything else.
    pub fn facts(&self) -> String {
        facts(self.year.as_deref(), self.source.as_deref(), self.kind.as_deref(), self.minutes)
    }

    /// The line under a landscape card's name: what it says of itself (Continue watching's "S1 E3 · 12m left"), else
    /// its facts.
    pub fn line(&self) -> String {
        card_line(self.sub.as_deref(), self.year.as_deref(), self.source.as_deref(), self.kind.as_deref(), self.minutes)
    }

    /// The line under a poster's name; `narrow`: the card is too narrow for its facts, so it gives the year (or what
    /// it is).
    pub fn poster_meta(&self, narrow: bool) -> String {
        poster_meta(self.year.as_deref(), self.source.as_deref(), self.kind.as_deref(), self.minutes, narrow)
    }
}

/// A landscape card's line (see [`Item::line`]).
pub fn card_line(sub: Option<&str>, year: Option<&str>, source: Option<&str>, kind: Option<&str>, minutes: Option<i64>) -> String {
    sub.map_or_else(|| facts(year, source, kind, minutes), String::from)
}

/// A poster's line (see [`Item::poster_meta`]).
pub fn poster_meta(year: Option<&str>, source: Option<&str>, kind: Option<&str>, minutes: Option<i64>, narrow: bool) -> String {
    if narrow {
        year.filter(|y| !y.is_empty()).map_or_else(|| what(source, kind).to_string(), String::from)
    } else {
        facts(year, source, kind, minutes)
    }
}

/// What a card calls a title of `source` and `kind` (see [`Item::what`]).
pub fn what(source: Option<&str>, kind: Option<&str>) -> &'static str {
    match (source, kind) {
        (Some("anime"), _) => "Anime",
        (_, Some("movie")) => "Movie",
        (Some("cinema"), _) => "TV Show",
        _ => "Drama",
    }
}

/// Portrait art with no writing on it (TMDB's, or ARY's big portrait art): a card writes its name over it.
pub fn clean_art(poster_clean: bool, source: Option<&str>, poster_hd: Option<&str>) -> bool {
    // (ARY's big portrait art is clean; its titles come with no source, as the server takes no source to mean ARY)
    poster_clean || (matches!(source, None | Some("ary")) && poster_hd.is_some_and(|p| !p.is_empty()))
}

/// Clean art, and a logo to carry over it.
pub fn own_art(clean_art: bool, logo: Option<&str>) -> bool {
    clean_art && logo.is_some_and(|l| !l.is_empty())
}

/// A year field's year as a number: its first four digits ("2021-10" is 2021).
pub fn year_number(year: Option<&str>) -> Option<i64> {
    year.and_then(|y| y.get(..4)).and_then(|y| y.parse().ok())
}

/// A card's facts (see [`Item::facts`]).
pub fn facts(year: Option<&str>, source: Option<&str>, kind: Option<&str>, minutes: Option<i64>) -> String {
    let length = minutes.filter(|m| *m > 0 && kind == Some("movie")).map(crate::title::runtime);
    let length = length.unwrap_or_else(|| what(source, kind).to_string());
    [year.filter(|y| !y.is_empty()), Some(length.as_str())].into_iter().flatten().collect::<Vec<_>>().join("  •  ")
}

/// A title's card, from its page: what Continue watching and New episodes dress a show's card with when the
/// catalogue's lists don't have it. Its cover is its backdrop with no writing (TMDB's), else the card-sized cut, and
/// an anime's metahub backdrop and logo when it has no clean art.
pub fn card_of(t: &Title) -> Item {
    let anime = if t.source == "anime" && !t.textless { metahub_art(t.poster.as_deref()) } else { None };
    let cover = anime.as_ref().map(|a| a.0.clone()).or_else(|| if t.textless { t.cover.clone() } else { t.cover_big.clone() }).or_else(|| t.cover.clone());
    Item {
        id: t.id.clone(),
        title: t.title.clone(),
        poster: t.poster.clone(),
        cover,
        poster_hd: t.poster_hd.clone(),
        logo: t.logo.clone().or_else(|| anime.map(|a| a.1)),
        source: Some(t.source.clone()).filter(|s| !s.is_empty()),
        kind: Some(t.kind.clone()).filter(|k| !k.is_empty()),
        poster_clean: t.poster_clean,
        textless: t.textless,
        available_at: t.available_at,
        ..Item::default()
    }
}

/// An episode on a title page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Episode {
    pub id: String,
    pub n: Option<i64>,
    pub season: Option<i64>,
    pub season_id: Option<String>,
    pub season_label: Option<String>,
    pub thumb: Option<String>,
    pub length: f64,
    pub date: Option<String>,
    pub stream: Option<String>,
    pub sprite: Option<String>,
    /// The episode's own name, when it has one.
    pub name: Option<String>,
    /// What happens in it, when the catalogue says.
    pub description: Option<String>,
}

/// A season on a title page; `extra`: specials, not counted as a season.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Season {
    pub id: String,
    pub label: String,
    pub extra: bool,
}

/// A title page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Title {
    pub id: String,
    pub title: String,
    pub source: String,
    pub kind: String,
    pub media_type: Option<String>,
    pub imdb: Option<String>,
    pub description: Option<String>,
    pub synopsis: Option<String>,
    pub cover: Option<String>,
    pub cover_big: Option<String>,
    pub poster: Option<String>,
    pub logo: Option<String>,
    pub rating: Option<String>,
    pub year: Option<String>,
    pub genres: Vec<String>,
    pub age: Option<String>,
    pub duration: Option<String>,
    /// TMDB's art: the cover has no writing on it, the logo is English.
    pub textless: bool,
    pub minutes: Option<i64>,
    pub stream: Option<String>,
    pub episodes: Vec<Episode>,
    pub seasons: Vec<Season>,
    pub cast: Vec<String>,
    /// A drama from abroad ("korean", "turkish"…): it plays in its own language unless the profile wants the dub.
    pub origin: Option<String>,
    /// Its poster is art with no writing on it (TMDB's), and ARY's big portrait art: what the poster cards show.
    pub poster_clean: bool,
    pub poster_hd: Option<String>,
    /// Not out yet: when it can be played (unix seconds). None: it plays now.
    pub available_at: Option<i64>,
    /// A film, not a show: one Download, no episodes to take all of.
    pub film: bool,
}

/// metahub's landscape backdrop and logo for the IMDb id in a metahub poster's address: what anime cards show (their
/// own covers are video stills with no name on them).
pub fn metahub_art(poster: Option<&str>) -> Option<(String, String)> {
    let p = poster?;
    if !p.contains("images.metahub.space/poster/") {
        return None;
    }
    let at = p.find("/tt")? + 1;
    let tt: String = p[at..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    (tt.len() > 3).then(|| {
        (
            format!("https://images.metahub.space/background/large/{tt}/img"),
            format!("https://images.metahub.space/logo/medium/{tt}/img"),
        )
    })
}

/// A card from the server's JSON. An anime card without its own logo gets metahub's backdrop and logo; its still stays
/// as what the card falls back to.
pub fn item(o: &Value) -> Item {
    let mut i = Item {
        id: t(o, "id"),
        title: t(o, "title"),
        poster: s(o, "poster"),
        cover: s(o, "cover"),
        cover_big: s(o, "coverBig"),
        fallback: s(o, "fallback"),
        poster_hd: s(o, "posterHD"),
        logo: s(o, "logo"),
        source: s(o, "source"),
        kind: s(o, "type"),
        episodes: i(o, "episodes"),
        year: s(o, "year"),
        genres: strings(o, "genres"),
        description: s(o, "description"),
        age: s(o, "age"),
        channel: s(o, "channel"),
        minutes: i(o, "minutes"),
        rating: s(o, "rating"),
        resume: None,
        sub: None,
        poster_clean: b(o, "posterClean"),
        textless: b(o, "textless"),
        origin: s(o, "origin"),
        new_at: i(o, "newAt"),
        new_kind: s(o, "newKind"),
        available_at: i(o, "availableAt"),
    };
    if i.source.as_deref() == Some("anime") && i.logo.is_none() && !i.textless {
        if let Some((backdrop, logo)) = metahub_art(i.poster.as_deref()) {
            i.cover_big = i.cover.take();
            i.cover = Some(backdrop);
            i.logo = Some(logo);
        }
    }
    i
}

/// The cards in `o[key]`.
pub fn items(o: &Value, key: &str) -> Vec<Item> {
    objects(o, key).into_iter().map(item).collect()
}

/// A title page from the server's JSON; `id`: the one asked for, used when the page has none.
pub fn title(id: &str, o: &Value) -> Title {
    Title {
        id: s(o, "id").unwrap_or_else(|| id.to_string()),
        title: t(o, "title"),
        source: t(o, "source"),
        kind: t(o, "type"),
        media_type: s(o, "mediaType"),
        imdb: s(o, "imdb"),
        description: s(o, "description"),
        synopsis: s(o, "synopsis"),
        cover: s(o, "cover"),
        cover_big: s(o, "coverBig"),
        poster: s(o, "poster"),
        logo: s(o, "logo"),
        rating: s(o, "rating"),
        year: s(o, "year"),
        genres: strings(o, "genres"),
        age: s(o, "age"),
        duration: s(o, "duration"),
        textless: b(o, "textless"),
        minutes: i(o, "minutes"),
        stream: s(o, "stream"),
        episodes: objects(o, "episodes").into_iter().map(episode).collect(),
        seasons: objects(o, "seasons")
            .iter()
            .map(|s_| Season { id: t(s_, "id"), label: t(s_, "label"), extra: b(s_, "extra") })
            .collect(),
        cast: strings(o, "cast"),
        origin: s(o, "origin"),
        poster_clean: b(o, "posterClean"),
        poster_hd: s(o, "posterHD"),
        available_at: i(o, "availableAt"),
        film: s(o, "type").as_deref() == Some("movie"),
    }
}

fn episode(e: &Value) -> Episode {
    Episode {
        id: t(e, "id"),
        n: i(e, "n"),
        season: i(e, "season"),
        season_id: s(e, "seasonId"),
        season_label: s(e, "seasonLabel"),
        thumb: s(e, "thumb"),
        length: d(e, "length"),
        date: s(e, "date"),
        stream: s(e, "stream"),
        sprite: s(e, "sprite"),
        name: s(e, "title"),
        description: s(e, "description").filter(|d| !d.trim().is_empty()),
    }
}

/// A field as text: None when missing, null or empty; a number or other value as its JSON.
pub(crate) fn s(o: &Value, key: &str) -> Option<String> {
    match o.get(key)? {
        Value::Null => None,
        Value::String(text) if text.is_empty() => None,
        Value::String(text) => Some(text.clone()),
        v => Some(v.to_string()),
    }
}

pub(crate) fn t(o: &Value, key: &str) -> String {
    s(o, key).unwrap_or_default()
}

/// A field as a whole number: a number (a fraction cut), or text of one.
pub(crate) fn i(o: &Value, key: &str) -> Option<i64> {
    match o.get(key)? {
        // (a fraction is cut toward zero, as the apps always read it)
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

pub(crate) fn d(o: &Value, key: &str) -> f64 {
    o.get(key).and_then(Value::as_f64).unwrap_or(0.)
}

pub(crate) fn b(o: &Value, key: &str) -> bool {
    o.get(key).and_then(Value::as_bool).unwrap_or(false)
}

pub(crate) fn objects<'a>(o: &'a Value, key: &str) -> Vec<&'a Value> {
    o.get(key).and_then(Value::as_array).map(|a| a.iter().filter(|v| v.is_object()).collect()).unwrap_or_default()
}

fn strings(o: &Value, key: &str) -> Vec<String> {
    o.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cards_read_leniently() {
        let card = item(&json!({"id": "x", "title": "X", "type": "movie", "minutes": 104.6, "year": 2026, "logo": "",
                                "genres": ["Drama", 3], "newAt": "1700000000", "posterHD": "p", "availableAt": 1800000000}));
        assert_eq!(card.minutes, Some(104));
        assert_eq!(card.year.as_deref(), Some("2026"));
        assert_eq!(card.logo, None);
        assert_eq!(card.genres, vec!["Drama".to_string(), "3".to_string()]);
        assert_eq!(card.new_at, Some(1_700_000_000));
        assert_eq!(card.available_at, Some(1_800_000_000));
        assert!(card.is_movie() && card.clean_art() && !card.own_art());
    }

    #[test]
    fn a_cards_lines() {
        let item = Item { year: Some("2023".into()), kind: Some("movie".into()), minutes: Some(105), ..Item::default() };
        assert_eq!(item.line(), "2023  •  1h 45m");
        assert_eq!(item.poster_meta(false), "2023  •  1h 45m");
        assert_eq!(item.poster_meta(true), "2023");
        let bare = Item { sub: Some("S1 E3 · 12m left".into()), ..Item::default() };
        assert_eq!(bare.line(), "S1 E3 · 12m left");
        assert_eq!(bare.poster_meta(true), "Drama");
    }

    #[test]
    fn what_a_card_says() {
        let film = item(&json!({"type": "movie", "year": "2023", "minutes": 105, "source": "cinema"}));
        assert_eq!((film.what(), film.facts().as_str()), ("Movie", "2023  •  1h 45m"));
        let show = item(&json!({"type": "show", "year": "2026", "source": "cinema"}));
        assert_eq!(show.facts(), "2026  •  TV Show");
        assert_eq!(item(&json!({"source": "anime", "type": "movie"})).facts(), "Anime");
        assert_eq!(item(&json!({})).what(), "Drama");
    }

    #[test]
    fn a_card_from_a_title_page() {
        let page = title("t", &json!({"title": "T", "source": "anime", "cover": "still", "coverBig": "big",
                                      "poster": "https://images.metahub.space/poster/medium/tt1234567/img"}));
        let card = card_of(&page);
        assert_eq!(card.cover.as_deref(), Some("https://images.metahub.space/background/large/tt1234567/img"));
        assert!(card.logo.is_some());
        let tmdb = card_of(&title("t", &json!({"source": "cinema", "cover": "clean", "coverBig": "big", "textless": true})));
        assert_eq!(tmdb.cover.as_deref(), Some("clean"));
        assert_eq!(card_of(&title("t", &json!({"source": "ary", "cover": "c", "coverBig": "big"}))).cover.as_deref(), Some("big"));
    }

    #[test]
    fn anime_cards_get_metahub_art() {
        let card = item(&json!({"id": "anime-a", "source": "anime", "cover": "still.jpg",
                                "poster": "https://images.metahub.space/poster/medium/tt1234567/img"}));
        assert_eq!(card.cover.as_deref(), Some("https://images.metahub.space/background/large/tt1234567/img"));
        assert_eq!(card.logo.as_deref(), Some("https://images.metahub.space/logo/medium/tt1234567/img"));
        assert_eq!(card.cover_big.as_deref(), Some("still.jpg"));
    }

    #[test]
    fn title_pages() {
        let page = title(
            "asked",
            &json!({"title": "Show", "type": "show", "seasons": [{"id": "s1", "label": "Season 1"}, {"id": "s0", "label": "Specials", "extra": true}],
                    "episodes": [{"id": "e1", "n": 1, "season": 1, "title": "Pilot", "description": "  ", "length": 1500.5}, "junk"],
                    "availableAt": "1800000000"}),
        );
        assert_eq!(page.id, "asked");
        assert_eq!(page.seasons.len(), 2);
        assert!(page.seasons[1].extra);
        assert_eq!(page.episodes.len(), 1);
        assert_eq!(page.episodes[0].name.as_deref(), Some("Pilot"));
        assert_eq!(page.episodes[0].description, None);
        assert_eq!(page.episodes[0].length, 1500.5);
        assert_eq!(page.available_at, Some(1_800_000_000));
    }
}
