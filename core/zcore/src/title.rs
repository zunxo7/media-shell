//! A title page's words and picks: its facts line, what Play says and does, its story and episodes, and the titles
//! like it.

use crate::home::Spot;

/// A length in minutes as the apps write it: "2h 28m", "45m".
pub fn runtime(minutes: i64) -> String {
    if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

/// The length a title's facts line shows: a film's runtime (or its duration, "1:52:00" as "1:52:00" without a leading
/// "00:"); a show's seasons when it has more than one (`seasons`: not counting specials), else its episodes.
pub fn length(
    is_movie: bool,
    minutes: Option<i64>,
    duration: Option<&str>,
    seasons: usize,
    episodes: usize,
) -> Option<String> {
    if is_movie {
        minutes
            .filter(|m| *m > 0)
            .map(runtime)
            .or_else(|| duration.filter(|d| *d != "00:00:00").map(|d| d.trim_start_matches("00:").to_string()))
    } else if seasons > 1 {
        Some(format!("{seasons} Seasons"))
    } else if episodes > 0 {
        Some(format!("{episodes} Episodes"))
    } else {
        None
    }
}

/// "2019   ·   2h 28m   ·   Sci-Fi": what's known of the year, the length and the first genre.
pub fn facts_line(year: Option<&str>, length: Option<&str>, genre: Option<&str>) -> String {
    [year.filter(|y| !y.is_empty()), length, genre].into_iter().flatten().collect::<Vec<_>>().join("   ·   ")
}

/// The last stretch of a video counted as its end credits (seconds): a spot inside it is finished. The server's
/// up-next (`server.py`, `FINISHED_TAIL`) uses the same number.
pub const CREDITS: f64 = 90.;

/// A saved spot still being watched: started, and more than the end credits left.
pub fn unfinished(at: f64, length: f64) -> bool {
    at > 0. && at < length - CREDITS
}

/// The spot Play picks up from in a title: the latest unfinished one whose key is one of `keys` (its episodes' ids,
/// and its own for a film), as an index into `spots`.
pub fn resume<S: AsRef<str>>(spots: &[Spot], keys: &[S]) -> Option<usize> {
    spots
        .iter()
        .enumerate()
        .filter(|(_, s)| unfinished(s.at, s.length) && keys.iter().any(|k| k.as_ref() == s.key))
        .max_by(|a, b| a.1.updated.total_cmp(&b.1.updated))
        .map(|(i, _)| i)
}

/// An episode as Play and the season chips see it: its id and the season it's in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EpisodeKey {
    pub id: String,
    pub season: Option<String>,
}

/// A season as the season chips see it; `extra`: specials.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeasonKey {
    pub id: String,
    pub extra: bool,
}

/// The season a title page opens on: the one `episode` (the spot being resumed) is in, else the first that isn't
/// specials, else the first.
pub fn season_shown(episodes: &[EpisodeKey], seasons: &[SeasonKey], episode: Option<&str>) -> Option<String> {
    episode
        .and_then(|id| episodes.iter().find(|e| e.id == id))
        .and_then(|e| e.season.clone())
        .or_else(|| seasons.iter().find(|s| !s.extra).or(seasons.first()).map(|s| s.id.clone()))
}

/// The episodes of the season showing, as indices: every one when the title has no seasons.
pub fn in_season(episodes: &[EpisodeKey], has_seasons: bool, season: Option<&str>) -> Vec<usize> {
    (0..episodes.len()).filter(|&i| !has_seasons || episodes[i].season.as_deref() == season).collect()
}

/// What Play says and does on a title page.
#[derive(Clone, Debug, PartialEq)]
pub struct Play {
    /// "Resume E3", "Play E1", "Resume", "Play".
    pub label: String,
    /// The episode it plays, as an index into the title's episodes; None for a film.
    pub episode: Option<usize>,
    /// The spot it resumes (and how far along the page shows), as an index into the spots.
    pub watching: Option<usize>,
}

/// Play on a title page (`id`), its season `season` showing. A show with seasons resumes the season shown (its
/// latest unfinished episode, else its first); a film resumes itself; any other show resumes where it was left, else
/// starts at its first episode.
pub fn play(id: &str, is_movie: bool, episodes: &[EpisodeKey], has_seasons: bool, season: Option<&str>, spots: &[Spot]) -> Play {
    let shown = in_season(episodes, has_seasons, season);
    let watching = if has_seasons {
        let keys: Vec<&str> = shown.iter().map(|&i| episodes[i].id.as_str()).collect();
        resume(spots, &keys)
    } else {
        let keys: Vec<&str> = episodes.iter().map(|e| e.id.as_str()).chain([id]).collect();
        resume(spots, &keys)
    };
    let spot = watching.map(|i| &spots[i]);
    let label = play_label(has_seasons, is_movie, spot.is_some(), spot.and_then(|s| s.ep));
    let episode = if has_seasons {
        spot.and_then(|s| shown.iter().copied().find(|&i| episodes[i].id == s.key)).or_else(|| shown.first().copied())
    } else if is_movie {
        None
    } else {
        spot.and_then(|s| episodes.iter().position(|e| e.id == s.key)).or((!episodes.is_empty()).then_some(0))
    };
    Play { label, episode, watching }
}

/// How far into a film, or the episode being watched: "E3  ·  12m left", "1h 5m left".
pub fn watching_line(is_movie: bool, ep: Option<i64>, at: f64, length: f64) -> String {
    let left = format!("{} left", runtime((((length - at) / 60.) as i64).max(1)));
    match ep.filter(|_| !is_movie) {
        Some(n) => format!("E{n}  ·  {left}"),
        None => left,
    }
}

/// What's left of a spot in the watch history: "Watched" once it's into the end credits, else "12m left".
pub fn left_words(at: f64, length: f64) -> String {
    if at >= length - CREDITS {
        "Watched".into()
    } else {
        format!("{} left", runtime((((length - at) / 60.) as i64).max(1)))
    }
}

/// A title's story, without the credits some sources tack on after it ("Written by…", "Directed by…", "Cast…").
pub fn story(description: Option<&str>, synopsis: Option<&str>) -> String {
    let text = description.or(synopsis).unwrap_or_default().trim();
    let cut = ["\nWritten", "\nDirected", "\nCast"].iter().filter_map(|m| text.find(m)).min().unwrap_or(text.len());
    text[..cut].trim().to_string()
}

/// An episode's name: its own, unless that's only "Episode 3"; then "Episode 3".
pub fn episode_name(name: Option<&str>, n: Option<i64>) -> String {
    let own = name.map(str::trim).filter(|name| {
        let lower = name.to_lowercase();
        let numbered = lower.strip_prefix("episode").is_some_and(|rest| rest.trim().chars().all(|c| c.is_ascii_digit()));
        !name.is_empty() && !numbered
    });
    match own {
        Some(own) => own.to_string(),
        None => format!("Episode {}", n.map(|n| n.to_string()).unwrap_or_default()),
    }
}

/// An episode's name with its number first, as a small row writes it: "3. Name"; the name alone when it has no number.
pub fn numbered_name(name: Option<&str>, n: Option<i64>) -> String {
    let name = episode_name(name, n);
    match n {
        Some(n) => format!("{n}. {name}"),
        None => name,
    }
}

/// An episode's length, to the nearest minute: "45m", "1h 2m"; "" when it isn't known.
pub fn episode_length(seconds: f64) -> String {
    if seconds > 0. {
        runtime(((seconds / 60.).round() as i64).max(1))
    } else {
        String::new()
    }
}

/// What Play says. A show with seasons resumes the season shown ("Resume E3", else "Play E1"); a film "Resume" or
/// "Play"; any other show resumes where it was left. `resuming`: there's an unfinished spot to go back to, at
/// episode `ep`.
pub fn play_label(has_seasons: bool, is_movie: bool, resuming: bool, ep: Option<i64>) -> String {
    let episode = || ep.map(|n| n.to_string()).unwrap_or_default();
    match (resuming, is_movie && !has_seasons) {
        (true, true) => "Resume".into(),
        (false, true) => "Play".into(),
        (true, false) => format!("Resume E{}", episode()),
        (false, false) => "Play E1".into(),
    }
}

/// How many titles "More like this" shows.
pub const RELATED: usize = 12;

/// The genres a Movies & TV title's "More like this" asks the catalogue for: up to two of the catalogue's own, the rarer
/// ones first (a broad one like Drama says little); none when it has none of them (then the catalogue's popular ones).
pub fn related_genres(genres: &[String]) -> Vec<String> {
    const GENRES: [&str; 19] = [
        "Action", "Adventure", "Animation", "Biography", "Comedy", "Crime", "Documentary", "Drama", "Family", "Fantasy",
        "History", "Horror", "Mystery", "Romance", "Sci-Fi", "Sport", "Thriller", "War", "Western",
    ];
    const BROAD: [&str; 7] = ["Drama", "Action", "Comedy", "Adventure", "Thriller", "Romance", "Crime"];
    let mut picks: Vec<&String> = genres.iter().filter(|g| GENRES.contains(&g.as_str())).collect();
    picks.sort_by_key(|g| BROAD.contains(&g.as_str()));
    picks.into_iter().take(2).cloned().collect()
}

/// Whether a card from `other` (its source) is on the same shelf as a title from `source` that isn't Movies & TV's:
/// anime with anime, the rest (ARY's and the channels' dramas) with each other.
pub fn same_shelf(source: &str, other: Option<&str>) -> bool {
    if source == "anime" {
        other == Some("anime")
    } else {
        !matches!(other, Some("anime") | Some("cinema"))
    }
}

/// A title as "More like this" weighs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub genres: Vec<String>,
    pub year: Option<i64>,
    pub kind: Option<String>,
}

/// The `n` titles of `pool` most like `title`, best first, as indices into `pool`: genre overlap weighted by how rare
/// each genre is in the pool, nudged by the same kind, a near year and the pool's own order.
pub fn related(title: &Candidate, pool: &[Candidate], n: usize) -> Vec<usize> {
    use std::collections::{HashMap, HashSet};
    let mine: HashSet<&String> = title.genres.iter().collect();
    if mine.is_empty() {
        return Vec::new();
    }
    let mut df: HashMap<&String, usize> = HashMap::new();
    for c in pool {
        for g in &c.genres {
            *df.entry(g).or_default() += 1;
        }
    }
    let idf = |g: &String| (((pool.len() + 1) as f64) / ((df.get(g).copied().unwrap_or(0) + 1) as f64)).ln() + 0.1;
    let mut seen = HashSet::from([title.id.as_str()]);
    let mut scored = Vec::new();
    for (rank, c) in pool.iter().enumerate() {
        if !seen.insert(c.id.as_str()) || c.genres.is_empty() {
            continue;
        }
        let theirs: HashSet<&String> = c.genres.iter().collect();
        let (mut shared, mut all) = (0., 0.);
        for g in mine.union(&theirs) {
            all += idf(g);
            if mine.contains(g) && theirs.contains(g) {
                shared += idf(g);
            }
        }
        if shared == 0. {
            continue;
        }
        let near = match (title.year, c.year) {
            (Some(a), Some(b)) => 0.12 * (1. - (b - a).abs() as f64 / 15.).max(0.),
            _ => 0.,
        };
        let same = if c.kind.is_some() && c.kind == title.kind { 0.15 } else { 0. };
        scored.push((shared / all + same + near + 0.08 * (1. - rank as f64 / pool.len() as f64), rank));
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().take(n).map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts() {
        assert_eq!(runtime(148), "2h 28m");
        assert_eq!(runtime(45), "45m");
        assert_eq!(length(true, Some(148), None, 0, 0).as_deref(), Some("2h 28m"));
        assert_eq!(length(true, None, Some("00:52:10"), 0, 0).as_deref(), Some("52:10"));
        assert_eq!(length(false, None, None, 3, 30).as_deref(), Some("3 Seasons"));
        assert_eq!(length(false, None, None, 1, 8).as_deref(), Some("8 Episodes"));
        assert_eq!(facts_line(Some("2019"), Some("2h 28m"), Some("Sci-Fi")), "2019   ·   2h 28m   ·   Sci-Fi");
        assert_eq!(facts_line(Some(""), None, Some("Drama")), "Drama");
    }

    #[test]
    fn play_says() {
        assert_eq!(play_label(true, false, true, Some(3)), "Resume E3");
        assert_eq!(play_label(true, false, false, None), "Play E1");
        assert_eq!(play_label(false, true, true, None), "Resume");
        assert_eq!(play_label(false, true, false, None), "Play");
        assert_eq!(play_label(false, false, true, Some(7)), "Resume E7");
        assert!(unfinished(100., 3000.) && !unfinished(2950., 3000.) && !unfinished(0., 3000.));
    }

    fn spot(key: &str, updated: f64, at: f64, ep: Option<i64>) -> Spot {
        Spot { key: key.into(), show: "show".into(), ep, updated, at, length: 3000., ..Spot::default() }
    }

    fn ep(id: &str, season: &str) -> EpisodeKey {
        EpisodeKey { id: id.into(), season: Some(season.into()) }
    }

    #[test]
    fn play_resumes_the_season_shown() {
        let episodes = vec![ep("a1", "s1"), ep("a2", "s1"), ep("b1", "s2"), ep("b2", "s2")];
        let spots = vec![spot("a2", 10., 500., Some(2)), spot("b1", 5., 2990., Some(1)), spot("x", 50., 100., None)];
        let s1 = play("show", false, &episodes, true, Some("s1"), &spots);
        assert_eq!(s1, Play { label: "Resume E2".into(), episode: Some(1), watching: Some(0) });
        // (season 2's only spot is finished: it starts at its first)
        let s2 = play("show", false, &episodes, true, Some("s2"), &spots);
        assert_eq!(s2, Play { label: "Play E1".into(), episode: Some(2), watching: None });
        let seasons = vec![SeasonKey { id: "s0".into(), extra: true }, SeasonKey { id: "s1".into(), extra: false }];
        assert_eq!(season_shown(&episodes, &seasons, Some("b2")).as_deref(), Some("s2"));
        assert_eq!(season_shown(&episodes, &seasons, None).as_deref(), Some("s1"));
    }

    #[test]
    fn play_for_films_and_plain_shows() {
        let film = play("film", true, &[], false, None, &[spot("film", 1., 600., None)]);
        assert_eq!(film, Play { label: "Resume".into(), episode: None, watching: Some(0) });
        assert_eq!(play("film", true, &[], false, None, &[]).label, "Play");
        let plain = vec![EpisodeKey { id: "e1".into(), season: None }, EpisodeKey { id: "e2".into(), season: None }];
        let show = play("show", false, &plain, false, None, &[spot("e2", 1., 600., Some(2)), spot("e1", 0., 600., Some(1))]);
        assert_eq!(show, Play { label: "Resume E2".into(), episode: Some(1), watching: Some(0) });
        assert_eq!(play("show", false, &plain, false, None, &[]).episode, Some(0));
        assert_eq!(resume(&[spot("e1", 1., 100., None)], &["e9"]), None);
    }

    #[test]
    fn words_on_a_title_page() {
        assert_eq!(watching_line(false, Some(3), 2280., 3000.), "E3  ·  12m left");
        assert_eq!(watching_line(true, Some(3), 0., 3900.), "1h 5m left");
        assert_eq!(watching_line(true, None, 2990., 3000.), "1m left");
        assert_eq!(left_words(2950., 3000.), "Watched");
        assert_eq!(left_words(0., 3900.), "1h 5m left");
        assert_eq!(story(Some(" A story.\nDirected by X\nWritten by Y "), None), "A story.");
        assert_eq!(story(None, Some("Only this.")), "Only this.");
        assert_eq!(numbered_name(Some("Pilot"), Some(3)), "3. Pilot");
        assert_eq!(numbered_name(Some("Pilot"), None), "Pilot");
    assert_eq!(episode_name(Some("Episode 3"), Some(3)), "Episode 3");
        assert_eq!(episode_name(Some(" The Return "), Some(3)), "The Return");
        assert_eq!(episode_name(Some("Episodes of Life"), None), "Episodes of Life");
        assert_eq!(episode_name(None, Some(4)), "Episode 4");
        assert_eq!(episode_length(2690.), "45m");
        assert_eq!(episode_length(7170.), "2h 0m");
        assert_eq!(episode_length(10.), "1m");
        assert_eq!(episode_length(0.), "");
    }

    #[test]
    fn more_like_this_asks_by_rarer_genres() {
        let g = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(related_genres(&g(&["Drama", "Horror", "Anime", "Mystery"])), g(&["Horror", "Mystery"]));
        assert_eq!(related_genres(&g(&["Drama"])), g(&["Drama"]));
        assert!(related_genres(&g(&["Reality"])).is_empty());
        assert!(same_shelf("anime", Some("anime")) && !same_shelf("anime", None));
        assert!(same_shelf("ary", None) && same_shelf("ary", Some("youtube")) && !same_shelf("ary", Some("cinema")));
    }

    fn c(id: &str, genres: &[&str], year: i64, kind: &str) -> Candidate {
        Candidate {
            id: id.into(),
            genres: genres.iter().map(|g| g.to_string()).collect(),
            year: Some(year),
            kind: Some(kind.into()),
        }
    }

    #[test]
    fn related_by_rare_genres() {
        let me = c("me", &["Horror", "Drama"], 2020, "movie");
        let pool = vec![
            c("me", &["Horror"], 2020, "movie"),
            c("drama", &["Drama"], 2020, "movie"),
            c("horror", &["Horror"], 2021, "movie"),
            c("none", &["Comedy"], 2020, "movie"),
            c("drama2", &["Drama"], 2019, "show"),
        ];
        assert_eq!(related(&me, &pool, 3), vec![1, 2, 4]);
    }
}
