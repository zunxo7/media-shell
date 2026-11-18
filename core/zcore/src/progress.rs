//! Saved spots: where each film or episode was left, as `/api/progress` sends them.

use crate::model::{d, i, s, t};
use serde_json::Value;
use std::collections::HashMap;

/// The server page that holds this profile's spots.
pub const PATH: &str = "/api/progress";

/// Where a film or an episode was left.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Saved {
    /// The episode's id, or the film's (then the same as `show`).
    pub key: String,
    /// The title it belongs to.
    pub show: String,
    pub title: String,
    pub ep: Option<i64>,
    pub cover: Option<String>,
    /// Where it was left, of how long it is (seconds).
    pub at: f64,
    pub length: f64,
    /// When it was saved (seconds since 1970).
    pub updated: f64,
}

/// The spots in `/api/progress`'s answer (an object by key); entries that aren't objects are skipped.
///
/// # Errors
/// [`crate::Error::BadAnswer`] when the answer isn't JSON.
pub fn saved(answer: &str) -> Result<Vec<Saved>, crate::Error> {
    let o: Value = serde_json::from_str(answer)?;
    let Some(all) = o.as_object() else { return Ok(Vec::new()) };
    Ok(all
        .iter()
        .filter(|(_, p)| p.is_object())
        .map(|(key, p)| Saved {
            key: s(p, "key").unwrap_or_else(|| key.clone()),
            show: t(p, "show"),
            title: t(p, "title"),
            ep: i(p, "ep"),
            cover: s(p, "cover"),
            at: d(p, "t"),
            length: d(p, "d"),
            updated: d(p, "updated"),
        })
        .collect())
}

/// Where a profile's spots are kept on the device.
pub fn key(profile: &str) -> String {
    crate::session::mine("progress", profile)
}

/// A profile's spots, by the key each is saved under.
pub type Spots = HashMap<String, Saved>;

/// The spots in `/api/progress`'s answer, by key; an answer that can't be read holds none.
pub fn spots(answer: &str) -> Spots {
    saved(answer).unwrap_or_default().into_iter().map(|s| (s.key.clone(), s)).collect()
}

/// The server's spots over those known here: a spot saved here after the server's copy of it (a page asked for before
/// the player saved) stays. One the server no longer has is gone (taken out of the history).
pub fn merge(known: &Spots, server: Spots) -> Spots {
    let mut all = server;
    for (key, theirs) in &mut all {
        if let Some(mine) = known.get(key).filter(|mine| mine.updated > theirs.updated) {
            *theirs = mine.clone();
        }
    }
    all
}

/// A spot just saved here, before the server's page has it.
pub fn remember(spots: &mut Spots, spot: Saved) {
    spots.insert(spot.key.clone(), spot);
}

impl Saved {
    /// As Home and the title page weigh it.
    pub fn spot(&self) -> crate::home::Spot {
        crate::home::Spot {
            key: self.key.clone(),
            show: self.show.clone(),
            title: self.title.clone(),
            ep: self.ep,
            cover: self.cover.clone(),
            at: self.at,
            length: self.length,
            updated: self.updated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spot_saved_here_outlasts_an_older_page() {
        let spot = |key: &str, at: f64, updated: f64| Saved { key: key.into(), at, updated, ..Saved::default() };
        let mut known = Spots::new();
        remember(&mut known, spot("e1", 300., 50.));
        remember(&mut known, spot("e2", 10., 5.));
        remember(&mut known, spot("gone", 10., 99.));
        let server = spots(r#"{"e1": {"t": 100, "updated": 40}, "e2": {"t": 20, "updated": 6}, "e3": {"t": 1}}"#);
        let all = merge(&known, server);
        assert_eq!(all.len(), 3);
        assert!(!all.contains_key("gone"));
        assert_eq!((all["e1"].at, all["e2"].at), (300., 20.));
        assert!(spots("<html>").is_empty());
        assert_eq!(key("p1"), "me-progress-p1");
    }

    #[test]
    fn reads_spots_leniently() {
        let answer = r#"{"e1": {"show": "s", "title": "Show", "ep": "3", "t": 60, "d": 1200.5, "updated": 9},
                         "m": {"key": "m", "show": "m", "title": "Film", "cover": ""}, "junk": 4}"#;
        let mut spots = saved(answer).unwrap_or_default();
        spots.sort_by(|a, b| a.key.cmp(&b.key));
        assert_eq!(spots.len(), 2);
        assert_eq!((spots[0].key.as_str(), spots[0].ep, spots[0].length), ("e1", Some(3), 1200.5));
        assert_eq!(spots[1].cover, None);
        assert!(saved("<html>").is_err());
    }
}
