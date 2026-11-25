//! Home: its rows as the server sends them, the tags its cards carry, and its own rows (Continue watching and New
//! episodes) made from this profile's saved spots and up-next.

use crate::model::{i, items, s, t, Item};
use crate::title::unfinished;
use serde_json::Value;

/// A row of cards.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    pub title: String,
    pub items: Vec<Item>,
    /// Landscape cards on the phone too (Continue watching).
    pub wide: bool,
    /// Its cards carry no New Episode / New Season / Recently Added tags: the server's word (its name already says
    /// what's new, or it's a genre), and the app's own rows.
    pub no_tags: bool,
    /// The Top 10: numbered places.
    pub ranked: bool,
    /// Continue watching: its cards play, and show how far in.
    pub continuing: bool,
}

/// Home as the server sends it: the stage's titles, the rows, every YouTube drama and every ARY title (Home shows
/// only a few of ARY's rows; the rest are for Dramas and search).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Home {
    pub hero: Vec<Item>,
    pub rows: Vec<Row>,
    pub youtube: Vec<Item>,
    pub ary: Vec<Item>,
}

impl Home {
    /// Every card Home knows, the stage's first: duplicates kept (callers pick by id).
    pub fn everything(&self) -> impl Iterator<Item = &Item> {
        self.hero.iter().chain(self.rows.iter().flat_map(|r| r.items.iter())).chain(self.youtube.iter()).chain(self.ary.iter())
    }
}

/// Home from the server's answer.
pub fn parse(o: &Value) -> Home {
    let rows = o
        .get("rows")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| r.is_object())
        .map(|r| Row {
            title: t(r, "title"),
            items: items(r, "items"),
            wide: false,
            no_tags: flag(r, "noTags"),
            ranked: flag(r, "ranked"),
            continuing: false,
        })
        .collect();
    let mut hero = items(o, "hero");
    hero.truncate(HERO_SLIDES);
    Home { hero, rows, youtube: items(o, "youtube"), ary: items(o, "ary") }
}

fn flag(o: &Value, key: &str) -> bool {
    o.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Where Home's lists are kept on the device.
pub const KEY: &str = "home";

/// The most titles the stage turns through: its row of small pictures stays readable.
pub const HERO_SLIDES: usize = 6;

/// How long the stage shows a title before the next (ms).
pub const HERO_STAY: u64 = 9_000;

// ---------- tags ----------

/// At most this many tagged cards in a row: more and they'd stop meaning anything.
pub const TAGS_PER_ROW: usize = 3;

/// A card's tag, told from its own day alone (the server's newAt and newKind; none without them): "New Episode" for
/// an episode out in the last 7 days, "New Season" for a season that premiered in the last 14, "Recently Added" for a
/// release in the last 30. `now`: unix seconds.
pub fn new_tag(new_at: Option<i64>, new_kind: Option<&str>, now: i64) -> Option<&'static str> {
    let age = now - new_at.filter(|&at| at > 0)?;
    let (words, days) = match new_kind? {
        "episode" => ("New Episode", 7),
        "season" => ("New Season", 14),
        "added" => ("Recently Added", 30),
        _ => return None,
    };
    // (a day's grace ahead: a day the server dates by its own clock)
    (-86_400..=days * 86_400).contains(&age).then_some(words)
}

/// The tags on a row's cards, in order: the first [`TAGS_PER_ROW`] cards that have one, none in a row kept plain.
pub fn row_tags(row: &Row, now: i64) -> Vec<Option<&'static str>> {
    let mut left = if row.no_tags { 0 } else { TAGS_PER_ROW };
    row.items
        .iter()
        .map(|i| {
            let tag = if left > 0 { new_tag(i.new_at, i.new_kind.as_deref(), now) } else { None };
            left -= usize::from(tag.is_some());
            tag
        })
        .collect()
}

// ---------- this profile's own rows ----------

/// Continue watching's row name.
pub const CONTINUE: &str = "Continue watching";

/// A saved spot: `key` the episode (or film) it's in, `show` its title, `ep` the episode's number, `cover` the
/// picture saved with it; where it is (`at`) of how long (`length`), and when it was saved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spot {
    pub key: String,
    pub show: String,
    pub title: String,
    pub ep: Option<i64>,
    pub cover: Option<String>,
    pub at: f64,
    pub length: f64,
    pub updated: f64,
}

/// Continue watching, as indices into `spots`: one card a show (the episode watched last; one finished leaves the show
/// to its next episode), newest first, twenty at most.
pub fn continue_watching(spots: &[Spot]) -> Vec<usize> {
    use std::collections::HashMap;
    let mut latest: HashMap<&str, usize> = HashMap::new();
    for (i, s) in spots.iter().enumerate() {
        if latest.get(s.show.as_str()).is_none_or(|&o| s.updated > spots[o].updated) {
            latest.insert(&s.show, i);
        }
    }
    let mut picked: Vec<usize> = latest.into_values().filter(|&i| unfinished(spots[i].at, spots[i].length)).collect();
    picked.sort_by(|&a, &b| spots[b].updated.total_cmp(&spots[a].updated));
    picked.truncate(20);
    picked
}

/// A show's next episode, from the server's up-next: `resume` the episode, `ep` its number, `new` how many episodes
/// came out since it was last watched.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpNext {
    pub id: String,
    pub title: String,
    pub cover: Option<String>,
    pub resume: Option<String>,
    pub ep: Option<i64>,
    pub new: i64,
}

/// The server's up-next answer.
pub fn up_next(o: &Value) -> Vec<UpNext> {
    o.get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|n| n.is_object())
        .map(|n| UpNext {
            id: t(n, "id"),
            title: t(n, "title"),
            cover: s(n, "cover"),
            resume: s(n, "resume"),
            ep: i(n, "ep"),
            new: i(n, "new").unwrap_or(0),
        })
        .collect()
}

/// A saved spot's episode as a card names it: "S2 E1" when its season is known (`season`), else "E1".
pub fn episode_short(season: Option<i64>, n: Option<i64>) -> Option<String> {
    let n = n?;
    Some(match season {
        Some(s) => format!("S{s} E{n}"),
        None => format!("E{n}"),
    })
}

/// A Continue watching card for an episode part-watched (`episode`: its short name); its art is dressed by [`dress`].
pub fn continue_card(spot: &Spot, episode: Option<&str>) -> Item {
    Item {
        id: spot.show.clone(),
        title: spot.title.clone(),
        cover: spot.cover.clone(),
        poster: spot.cover.clone(),
        resume: Some(spot.key.clone()),
        sub: Some(left_line(episode, spot.at, spot.length)),
        ..Item::default()
    }
}

/// A Continue watching card for a show's next episode, out since (`episode`: its short name); dressed by [`dress`].
pub fn up_next_card(next: &UpNext, episode: Option<&str>) -> Item {
    Item {
        id: next.id.clone(),
        title: next.title.clone(),
        cover: next.cover.clone(),
        poster: next.cover.clone(),
        resume: next.resume.clone(),
        sub: Some(up_next_line(episode, next.new)),
        ..Item::default()
    }
}

/// A Continue watching card in its show's own art (`known`: the show's card in the catalogue): its
/// backdrop and logo (the episode's still kept to fall back on), its poster, and what the art says about itself.
pub fn dress(card: Item, known: &Item) -> Item {
    Item {
        cover_big: if known.cover.is_some() { card.cover.clone() } else { card.cover_big.clone() },
        cover: known.cover.clone().or(card.cover),
        logo: known.logo.clone().or(card.logo),
        poster: known.poster.clone().or(card.poster),
        poster_hd: known.poster_hd.clone(),
        poster_clean: known.poster_clean,
        textless: known.textless,
        source: known.source.clone().or(card.source),
        available_at: known.available_at,
        ..card
    }
}

/// Continue watching as a row (none when empty), to go above the server's: the episodes part-watched and the shows
/// whose next episode is out, each with when it was last watched, newest first, twenty at most; a show once (its
/// part-watched episode first).
pub fn own_rows(cards: Vec<(f64, Item)>) -> Vec<Row> {
    let mut cards = cards;
    cards.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut seen = std::collections::HashSet::new();
    let items: Vec<Item> = cards.into_iter().map(|(_, item)| item).filter(|item| seen.insert(item.id.clone())).take(20).collect();
    let row = Row { title: CONTINUE.into(), items, wide: true, no_tags: true, ranked: false, continuing: true };
    if row.items.is_empty() { vec![] } else { vec![row] }
}

/// A Continue watching card's line: "S2 E1 · 12m left" (`episode`: the episode's short name, when it has one).
pub fn left_line(episode: Option<&str>, at: f64, length: f64) -> String {
    format!("{}{}m left", lead(episode), (((length - at) / 60.) as i64).max(1))
}

/// A next episode's line: "S2 E4 · 3 new episodes", "New episode", or "Up next" (`new`: episodes out since).
pub fn up_next_line(episode: Option<&str>, new: i64) -> String {
    let what = match new {
        n if n > 1 => format!("{n} new episodes"),
        1 => "New episode".to_string(),
        _ => "Up next".to_string(),
    };
    format!("{}{what}", lead(episode))
}

fn lead(episode: Option<&str>) -> String {
    episode.map(|e| format!("{e} · ")).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spot(show: &str, updated: f64, at: f64) -> Spot {
        Spot { key: format!("{show}-e"), show: show.into(), updated, at, length: 3000., ..Spot::default() }
    }

    #[test]
    fn one_card_a_show_newest_first_finished_left_out() {
        let spots = vec![
            spot("a", 10., 500.),
            spot("a", 20., 900.),  // (a's latest)
            spot("b", 30., 2990.), // (finished: left to New episodes)
            spot("c", 15., 100.),
        ];
        assert_eq!(continue_watching(&spots), vec![1, 3]);
    }

    #[test]
    fn lines() {
        assert_eq!(left_line(Some("S2 E1"), 2280., 3000.), "S2 E1 · 12m left");
        assert_eq!(left_line(None, 2990., 3000.), "1m left");
        assert_eq!(up_next_line(Some("E4"), 3), "E4 · 3 new episodes");
        assert_eq!(up_next_line(None, 1), "New episode");
        assert_eq!(up_next_line(None, 0), "Up next");
        assert_eq!(episode_short(Some(2), Some(1)).as_deref(), Some("S2 E1"));
        assert_eq!(episode_short(None, Some(1)).as_deref(), Some("E1"));
        assert_eq!(episode_short(Some(2), None), None);
    }

    #[test]
    fn rows_follow_the_server() {
        let home = parse(&json!({"hero": [{"id": "h"}], "rows": [
            {"title": "Top 10 on zunflix", "items": [{"id": "a"}], "noTags": true, "ranked": true},
            {"title": "Dramas", "items": [{"id": "b"}]}, "junk"], "ary": [{"id": "c"}]}));
        assert_eq!(home.rows.len(), 2);
        assert!(home.rows[0].ranked && home.rows[0].no_tags && !home.rows[1].ranked && !home.rows[1].no_tags);
        assert_eq!(home.everything().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec!["h", "a", "b", "c"]);
    }

    #[test]
    fn the_stage_holds_six() {
        let hero: Vec<_> = (0..9).map(|i| json!({"id": i.to_string()})).collect();
        assert_eq!(parse(&json!({"hero": hero})).hero.len(), HERO_SLIDES);
    }

    #[test]
    fn tags_by_their_own_day_three_a_row() {
        let now = 1_000_000_000;
        assert_eq!(new_tag(Some(now - 6 * 86_400), Some("episode"), now), Some("New Episode"));
        assert_eq!(new_tag(Some(now - 8 * 86_400), Some("episode"), now), None);
        assert_eq!(new_tag(Some(now - 13 * 86_400), Some("season"), now), Some("New Season"));
        assert_eq!(new_tag(Some(now - 29 * 86_400), Some("added"), now), Some("Recently Added"));
        assert_eq!(new_tag(Some(now + 2 * 86_400), Some("added"), now), None);
        assert_eq!(new_tag(None, Some("added"), now), None);
        let card = |id: &str| Item { id: id.into(), new_at: Some(now), new_kind: Some("added".into()), ..Item::default() };
        let mut row = Row { items: vec![card("a"), Item::default(), card("b"), card("c"), card("d")], ..Row::default() };
        let tags = row_tags(&row, now);
        assert_eq!(tags.iter().filter(|t| t.is_some()).count(), 3);
        assert_eq!(tags[1], None);
        assert_eq!(tags[4], None);
        row.no_tags = true;
        assert!(row_tags(&row, now).iter().all(Option::is_none));
    }

    #[test]
    fn own_rows_wear_their_shows_art() {
        let s = Spot { key: "e3".into(), show: "show".into(), title: "Show".into(), ep: Some(3), cover: Some("still".into()), at: 2280., length: 3000., updated: 1. };
        let card = continue_card(&s, Some("S1 E3"));
        assert_eq!((card.id.as_str(), card.resume.as_deref(), card.sub.as_deref()), ("show", Some("e3"), Some("S1 E3 · 12m left")));
        let known = Item { cover: Some("backdrop".into()), logo: Some("logo".into()), poster: Some("poster".into()), poster_hd: Some("hd".into()),
                           poster_clean: true, textless: true, source: Some("cinema".into()), available_at: Some(1), ..Item::default() };
        let dressed = dress(card, &known);
        assert_eq!((dressed.cover.as_deref(), dressed.cover_big.as_deref()), (Some("backdrop"), Some("still")));
        assert_eq!((dressed.logo.as_deref(), dressed.poster.as_deref(), dressed.poster_hd.as_deref()), (Some("logo"), Some("poster"), Some("hd")));
        assert!(dressed.poster_clean && dressed.textless && dressed.available_at == Some(1) && dressed.resume.is_some());
        let next = up_next(&json!({"items": [{"id": "x", "title": "X", "cover": "c", "resume": "x2", "ep": 2, "new": 3}]}));
        assert_eq!(next[0], UpNext { id: "x".into(), title: "X".into(), cover: Some("c".into()), resume: Some("x2".into()), ep: Some(2), new: 3 });
        let rows = own_rows(vec![(1., dressed)]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].continuing && rows[0].wide && rows[0].no_tags && rows[0].title == CONTINUE);
        // (a show whose next episode is out goes in with them, by when it was last watched; a show only once)
        let mut later = up_next_card(&next[0], Some("E2"));
        later.sub = Some(up_next_line(Some("E2"), 3));
        let mut again = continue_card(&s, None);
        again.id = "x".into();
        let rows = own_rows(vec![(1., continue_card(&s, None)), (5., later.clone()), (2., again)]);
        assert_eq!(rows[0].items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["x", "show"]);
        assert_eq!(rows[0].items[0].sub.as_deref(), Some("E2 · 3 new episodes"));
        assert!(own_rows(vec![]).is_empty());
    }
}
