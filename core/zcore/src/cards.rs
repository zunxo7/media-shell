//! A card as every page draws it: the title, the tag it wears (what's new about it), whether it's in the Top 10, how
//! far into it you are; and what pressing it does.

use crate::home::{row_tags, Row};
use crate::model::Item;
use crate::screen::{Effect, Route};
use std::collections::{HashMap, HashSet};

/// A card, with everything its page needs to draw it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    pub item: Item,
    /// "Coming 15 Mar", "New Episode", "New Season", "Recently Added".
    pub tag: Option<String>,
    /// One of Home's Top 10: the badge.
    pub top10: bool,
    /// How far into it, 0 to 1 (Continue watching's cards); None draws no bar.
    pub progress: Option<f32>,
}

/// The Top 10 mark's two lines: a tiny label over a bigger number.
pub const TOP_LABEL: &str = "TOP";
pub const TOP_NUMBER: &str = "10";

/// What pressing a card does: one carrying on (Continue watching, New episodes) plays; any other opens its page.
pub fn tap(item: &Item) -> Effect {
    match &item.resume {
        Some(resume) => Effect::Play { show: item.id.clone(), episode: (resume != &item.id).then(|| resume.clone()) },
        None => Effect::Open { route: Route::Title { id: item.id.clone() } },
    }
}

/// What Home's rows say about cards anywhere: which are in the Top 10, and each one's day, for the grids.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marks {
    top10: HashSet<String>,
    days: HashMap<String, (Option<i64>, Option<String>)>,
}

impl Marks {
    /// The marks Home's rows (as the server sends them) give.
    pub fn new(rows: &[Row]) -> Self {
        let mut marks = Self::default();
        for row in rows {
            if row.ranked {
                marks.top10.extend(row.items.iter().map(|i| i.id.clone()));
            }
            for i in row.items.iter().filter(|i| i.new_at.is_some()) {
                marks.days.entry(i.id.clone()).or_insert_with(|| (i.new_at, i.new_kind.clone()));
            }
        }
        marks
    }

    /// A row's cards, its tags by the row's own rule ([`row_tags`]); `now`: unix seconds.
    pub fn row(&self, row: &Row, now: i64) -> Vec<Card> {
        row.items
            .iter()
            .zip(row_tags(row, now))
            .map(|(item, tag)| Card {
                item: item.clone(),
                tag: coming(item, now).or(tag.map(String::from)),
                top10: self.top10.contains(&item.id),
                progress: None,
            })
            .collect()
    }

    /// A card in a grid (a section, search): its tag from its own day, or the day Home's rows know it by.
    pub fn grid(&self, item: &Item, now: i64) -> Card {
        let (at, kind) = self.days.get(&item.id).cloned().unwrap_or_else(|| (item.new_at, item.new_kind.clone()));
        Card {
            item: item.clone(),
            tag: coming(item, now).or(crate::home::new_tag(at, kind.as_deref(), now).map(String::from)),
            top10: self.top10.contains(&item.id),
            progress: None,
        }
    }
}

/// A card not out yet wears its day instead.
fn coming(item: &Item, now: i64) -> Option<String> {
    crate::dates::coming(item.available_at, now as f64, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> Item {
        Item { id: id.into(), ..Item::default() }
    }

    #[test]
    fn carrying_on_plays_and_anything_else_opens() {
        let film = Item { resume: Some("m".into()), ..item("m") };
        assert_eq!(tap(&film), Effect::Play { show: "m".into(), episode: None });
        let show = Item { resume: Some("e3".into()), ..item("s") };
        assert_eq!(tap(&show), Effect::Play { show: "s".into(), episode: Some("e3".into()) });
        assert_eq!(tap(&item("x")), Effect::Open { route: Route::Title { id: "x".into() } });
    }

    #[test]
    fn grids_borrow_homes_top_10_and_days() {
        let now = 1_000_000;
        let fresh = Item { new_at: Some(now - 10), new_kind: Some("added".into()), ..item("a") };
        let rows = vec![
            Row { ranked: true, no_tags: true, items: vec![item("t")], ..Row::default() },
            Row { items: vec![fresh], ..Row::default() },
        ];
        let marks = Marks::new(&rows);
        assert!(marks.grid(&item("t"), now).top10);
        assert_eq!(marks.grid(&item("a"), now).tag.as_deref(), Some("Recently Added"));
        assert_eq!(marks.row(&rows[0], now)[0].tag, None);
        assert_eq!(marks.row(&rows[1], now)[0].tag.as_deref(), Some("Recently Added"));
        assert_eq!(marks.grid(&item("z"), now), Card { item: item("z"), ..Card::default() });
        let soon = Item { available_at: Some(now + 86_400), ..item("c") };
        assert!(marks.grid(&soon, now).tag.is_some_and(|t| t.starts_with("Coming ")));
    }
}
