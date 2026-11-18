//! The sections (Dramas, TV Shows, Movies, Anime): their chips, where each chip's list comes from, how a list is
//! put together from what the app fetched, and which titles a section's search keeps to.

use crate::model::{year_number, Item};
use std::collections::HashSet;

/// Where a chip's list comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Load {
    /// The Movies & TV catalogue (`/api/cinema`): its type ("movie", "series"), collection and genre ("" for all).
    Cinema { kind: &'static str, collection: &'static str, genre: &'static str },
    /// ARY's own titles (and, not for films, the channels' YouTube dramas), from Home: [`from_home`].
    FromHome { movies: bool },
    /// Dramas from everywhere: Pakistan's from Home and the [`DRAMA_LANDS`]' from the catalogue, dealt in turn
    /// ([`dramas`]); `genre` narrows them all.
    Dramas { genre: &'static str, newest: bool },
    /// The anime list ([`anime`]).
    Anime { genre: &'static str, newest: bool },
}

/// A section's chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tab {
    pub key: &'static str,
    pub label: &'static str,
    pub load: Load,
}

/// The countries whose dramas the Dramas section lists beside Pakistan's (no Turkish dramas: the owner's call).
pub const DRAMA_LANDS: [&str; 1] = ["korean"];

/// What a section says while the server puts a list together for the first time.
pub const BUILDING: &str = "Putting this list together. It'll be here in a few minutes.";

const SHOW_GENRES: [&str; 8] = ["Crime", "Drama", "Comedy", "Thriller", "Sci-Fi", "Mystery", "Action", "Fantasy"];
const MOVIE_GENRES: [&str; 8] = ["Action", "Comedy", "Drama", "Thriller", "Horror", "Sci-Fi", "Romance", "Animation"];
const ANIME_GENRES: [&str; 7] = ["Action", "Comedy", "Romance", "Fantasy", "Adventure", "Slice of Life", "Supernatural"];
const DRAMA_GENRES: [&str; 6] = ["Romance", "Family", "Comedy", "Thriller", "Crime", "Mystery"];

const fn tab(key: &'static str, label: &'static str, load: Load) -> Tab {
    Tab { key, label, load }
}

const fn cinema(kind: &'static str, collection: &'static str, genre: &'static str) -> Load {
    Load::Cinema { kind, collection, genre }
}

/// A section's chips (`section`: "dramas", "shows", "movies", anything else anime), one pattern: All (what's
/// popular), Latest (the newest), a few of its own, then its main genres.
pub fn tabs(section: &str) -> Vec<Tab> {
    let (mut list, genres): (Vec<Tab>, &[&'static str]) = match section {
        "dramas" => (
            vec![
                tab("all", "All", Load::Dramas { genre: "", newest: false }),
                tab("latest", "Latest", Load::Dramas { genre: "", newest: true }),
                tab("pakistani", "Pakistani", Load::FromHome { movies: false }),
                tab("korean", "Korean", cinema("series", "korean", "")),
            ],
            &DRAMA_GENRES,
        ),
        "shows" => (
            vec![
                tab("all", "All", cinema("series", "popular", "")),
                tab("latest", "Latest", cinema("series", "year", "")),
                tab("featured", "Top rated", cinema("series", "featured", "")),
            ],
            &SHOW_GENRES,
        ),
        "movies" => (
            vec![
                tab("all", "All", cinema("movie", "popular", "")),
                tab("latest", "Latest", cinema("movie", "year", "")),
                tab("featured", "Top rated", cinema("movie", "featured", "")),
                tab("marvel", "Marvel", cinema("movie", "marvel", "")),
                tab("pakistani", "Pakistani", Load::FromHome { movies: true }),
            ],
            &MOVIE_GENRES,
        ),
        _ => (
            vec![tab("all", "All", Load::Anime { genre: "", newest: false }), tab("latest", "Latest", Load::Anime { genre: "", newest: true })],
            &ANIME_GENRES,
        ),
    };
    for &genre in genres {
        let load = match section {
            "dramas" => Load::Dramas { genre, newest: false },
            "shows" => cinema("series", "popular", genre),
            "movies" => cinema("movie", "popular", genre),
            _ => Load::Anime { genre, newest: false },
        };
        list.push(tab(genre, genre, load));
    }
    list
}

/// A section's name.
pub fn title(section: &str) -> &'static str {
    match section {
        "dramas" => "Dramas",
        "shows" => "TV Shows",
        "movies" => "Movies",
        "manhwa" => "Manhwa",
        _ => "Anime",
    }
}

/// A Movies & TV catalogue page's address, `skip` titles in. "year" is the newest first: `year` (this year's, from the
/// app's clock) says which.
pub fn cinema_path(kind: &str, collection: &str, genre: &str, year: i32, skip: i64) -> String {
    let genre = if genre.is_empty() { String::new() } else { format!("&genre={}", encode(genre)) };
    let year = if collection == "year" { format!("&year={year}") } else { String::new() };
    format!("/api/cinema?type={kind}&collection={collection}{genre}{year}&skip={skip}")
}

/// "More like this"'s ask of the catalogue: titles of `kind` in `genre` ("" for the popular ones).
pub fn related_path(kind: &str, genre: &str) -> String {
    let genre = if genre.is_empty() { String::new() } else { format!("&genre={}", encode(genre)) };
    format!("/api/cinema?type={kind}{genre}")
}

/// ARY's own titles (`movies`: its films, else its dramas, the channels' YouTube ones among them) from Home's cards,
/// once each, by name.
pub fn from_home<'a>(cards: impl IntoIterator<Item = &'a Item>, movies: bool) -> Vec<Item> {
    let mut seen = HashSet::new();
    let mut list: Vec<Item> = cards
        .into_iter()
        .filter(|i| !matches!(i.source.as_deref(), Some("anime") | Some("cinema")) && i.is_movie() == movies)
        .filter(|i| seen.insert(i.id.clone()))
        .cloned()
        .collect();
    list.sort_by_cached_key(|i| i.title.to_lowercase());
    list
}

/// Whether a card is in `genre` ("" is every genre).
pub fn in_genre(item: &Item, genre: &str) -> bool {
    genre.is_empty() || item.genres.iter().any(|g| g.eq_ignore_ascii_case(genre))
}

/// Several lists dealt out in turn (one from each, then the next from each), once each, so no one of them fills the
/// page first.
pub fn dealt(lists: &[Vec<Item>]) -> Vec<Item> {
    let most = lists.iter().map(Vec::len).max().unwrap_or(0);
    let mut seen = HashSet::new();
    (0..most).flat_map(|i| lists.iter().filter_map(move |l| l.get(i))).filter(|i| seen.insert(i.id.as_str())).cloned().collect()
}

/// The newest first, by year (the order kept within a year).
pub fn newest_first(list: &mut [Item]) {
    list.sort_by_key(|i| std::cmp::Reverse(year_number(i.year.as_deref()).unwrap_or(0)));
}

/// Dramas: Pakistan's (`home`: from [`from_home`]) and each of the [`DRAMA_LANDS`]' lists (`abroad`, already asked in
/// `genre`), dealt in turn.
pub fn dramas(home: Vec<Item>, abroad: Vec<Vec<Item>>, genre: &str, newest: bool) -> Vec<Item> {
    let mine: Vec<Item> = home.into_iter().filter(|i| in_genre(i, genre)).collect();
    let mut all = dealt(&[vec![mine], abroad].concat());
    if newest {
        newest_first(&mut all);
    }
    all
}

/// The anime list, in `genre`, maybe newest first.
pub fn anime(list: &[Item], genre: &str, newest: bool) -> Vec<Item> {
    let mut all: Vec<Item> = list.iter().filter(|i| in_genre(i, genre)).cloned().collect();
    if newest {
        newest_first(&mut all);
    }
    all
}

/// Whether a title belongs in a section's search (`scope` None: every section).
pub fn in_scope(item: &Item, scope: Option<&str>) -> bool {
    let movie = item.is_movie();
    let source = item.source.as_deref();
    match scope {
        Some("anime") => source == Some("anime"),
        Some("movies") => movie && source != Some("anime"),
        Some("shows") => source == Some("cinema") && !movie,
        // (what the Dramas page holds: Pakistan's, and the dramas from the lands it lists; not every show called a drama)
        Some("dramas") => {
            !movie
                && match source {
                    Some("anime") => false,
                    Some("cinema") => item.origin.as_deref().is_some_and(|o| DRAMA_LANDS.contains(&o)),
                    _ => true,
                }
        }
        _ => true,
    }
}

/// A query's value as a form encodes it: letters, digits and `-_.*` as they are, a space `+`.
pub(crate) fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, source: Option<&str>, kind: &str, year: &str, genres: &[&str]) -> Item {
        Item {
            id: id.into(),
            title: id.to_uppercase(),
            source: source.map(String::from),
            kind: Some(kind.into()),
            year: Some(year.into()),
            genres: genres.iter().map(|g| g.to_string()).collect(),
            ..Item::default()
        }
    }

    #[test]
    fn chips_follow_one_pattern() {
        let dramas = tabs("dramas");
        assert_eq!(dramas.iter().map(|t| t.key).take(4).collect::<Vec<_>>(), ["all", "latest", "pakistani", "korean"]);
        assert_eq!(dramas.len(), 4 + 6);
        assert_eq!(tabs("movies")[3].load, Load::Cinema { kind: "movie", collection: "marvel", genre: "" });
        assert_eq!(tabs("shows").last().map(|t| t.load), Some(Load::Cinema { kind: "series", collection: "popular", genre: "Fantasy" }));
        assert_eq!(tabs("anime")[1].load, Load::Anime { genre: "", newest: true });
        assert_eq!(title("shows"), "TV Shows");
    }

    #[test]
    fn addresses() {
        assert_eq!(cinema_path("movie", "year", "", 2026, 40), "/api/cinema?type=movie&collection=year&year=2026&skip=40");
        assert_eq!(cinema_path("series", "popular", "Sci-Fi", 2026, 0), "/api/cinema?type=series&collection=popular&genre=Sci-Fi&skip=0");
        assert_eq!(related_path("movie", "Slice of Life"), "/api/cinema?type=movie&genre=Slice+of+Life");
    }

    #[test]
    fn lists_put_together() {
        let home = [card("b", None, "show", "2020", &["Romance"]), card("a", Some("youtube"), "show", "2024", &[]),
                    card("m", None, "movie", "2021", &[]), card("x", Some("cinema"), "show", "2022", &[]), card("b", None, "show", "2020", &[])];
        let ary = from_home(&home, false);
        assert_eq!(ary.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(from_home(&home, true).len(), 1);
        let abroad = vec![card("k1", Some("cinema"), "series", "2023", &[]), card("k2", Some("cinema"), "series", "2025", &[])];
        let all = dramas(ary.clone(), vec![abroad.clone()], "", false);
        assert_eq!(all.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["a", "k1", "b", "k2"]);
        let newest = dramas(ary.clone(), vec![abroad], "", true);
        assert_eq!(newest[0].id, "k2");
        assert_eq!(dramas(ary, vec![], "romance", false).len(), 1);
        let list = [card("o", Some("anime"), "show", "2001", &["Action"]), card("n", Some("anime"), "show", "2020", &[])];
        assert_eq!(anime(&list, "", true)[0].id, "n");
        assert_eq!(anime(&list, "action", false).len(), 1);
    }

    #[test]
    fn a_sections_search_keeps_to_it() {
        let korean = Item { origin: Some("korean".into()), ..card("k", Some("cinema"), "series", "", &[]) };
        let turkish = Item { origin: Some("turkish".into()), ..card("t", Some("cinema"), "series", "", &[]) };
        assert!(in_scope(&korean, Some("dramas")) && !in_scope(&turkish, Some("dramas")));
        assert!(in_scope(&card("p", None, "show", "", &[]), Some("dramas")));
        assert!(in_scope(&turkish, Some("shows")) && !in_scope(&korean, Some("movies")));
        assert!(in_scope(&card("f", Some("cinema"), "movie", "", &[]), Some("movies")));
        assert!(!in_scope(&card("a", Some("anime"), "movie", "", &[]), Some("movies")));
        assert!(in_scope(&turkish, None));
    }
}
