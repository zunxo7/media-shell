//! What can be skipped in a film or an episode of Movies & TV, from TheIntroDB. Apps fetch the answer themselves
//! (its free limit, 5000 a day, is per address, so each device asks for itself) and the core reads it.
//!
//! Verified timings only: submissions at least three people agree on with confidence 0.3 or more, for a release
//! within 30 s of the length of the copy that's playing, and at least 5 s long. With no verified end credits, where one
//! or two people say they start still counts (kind "end"), unless it's before the last quarter (a mislabelled intro).

use serde_json::Value;

use crate::player::Segment;

/// The TheIntroDB address for a title (`imdb`, plus `season` and `episode` for an episode) whose copy is `length`
/// seconds long; None when `imdb` isn't an IMDb id.
pub fn url(imdb: &str, season: Option<i64>, episode: Option<i64>, length: f64) -> Option<String> {
    let digits = imdb.strip_prefix("tt")?;
    if !(7..=8).contains(&digits.len()) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let tv = match season.zip(episode) {
        Some((s, n)) => format!("&season={s}&episode={n}"),
        None => String::new(),
    };
    Some(format!(
        "https://api.theintrodb.org/v3/media?imdb_id={imdb}{tv}&duration_ms={}&details=true",
        (length * 1000.) as i64
    ))
}

/// The skip segments in TheIntroDB's `answer` for a copy `length` seconds long, by start. Empty when the answer is
/// for another cut, or can't be read.
pub fn segments(answer: &str, length: f64) -> Vec<Segment> {
    let Ok(data) = serde_json::from_str::<Value>(answer) else {
        return Vec::new();
    };
    let release = data["group_duration_ms"].as_f64().unwrap_or(0.) / 1000.;
    if release <= 0. || (release - length).abs() > 30. {
        return Vec::new();
    }
    let ms = |o: &Value, key: &str| o[key].as_f64().map(|n| n / 1000.);
    let list = |kind: &str| data[kind].as_array().cloned().unwrap_or_default();
    let mut found = Vec::new();
    for kind in ["recap", "intro", "credits"] {
        for o in list(kind) {
            if o["submission_count"].as_i64().unwrap_or(0) < 3 || o["confidence"].as_f64().unwrap_or(0.) < 0.3 {
                continue;
            }
            let start = match ms(&o, "start_ms") {
                Some(s) => s,
                None if kind == "credits" => continue,
                None => 0.,
            };
            // (an end of 0 means it has none)
            let end = ms(&o, "end_ms").filter(|e| *e > 0.);
            if kind != "credits" && end.is_none() {
                continue;
            }
            if start >= length - 5. || end.is_some_and(|e| e - start < 5.) {
                continue;
            }
            found.push(Segment { kind: kind.into(), start, end: end.filter(|e| *e < length - 2.) });
        }
    }
    if !found.iter().any(|s| s.kind == "credits" && s.end.is_none()) {
        let start = list("credits")
            .iter()
            .filter(|o| o["end_ms"].is_null())
            .filter_map(|o| ms(o, "start_ms"))
            .filter(|s| *s >= length * 0.75 && *s < length - 10.)
            .reduce(f64::max);
        if let Some(start) = start {
            found.push(Segment { kind: "end".into(), start, end: None });
        }
    }
    found.sort_by(|a, b| a.start.total_cmp(&b.start));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const LENGTH: f64 = 2600.;

    fn answer(body: &str) -> String {
        format!(r#"{{"group_duration_ms": 2610000, {body}}}"#)
    }

    #[test]
    fn addresses() {
        assert_eq!(
            url("tt1234567", Some(2), Some(3), 2600.5).as_deref(),
            Some("https://api.theintrodb.org/v3/media?imdb_id=tt1234567&season=2&episode=3&duration_ms=2600500&details=true")
        );
        assert!(url("tt123", None, None, 1.).is_none());
        assert!(url("nm1234567", None, None, 1.).is_none());
    }

    #[test]
    fn verified_segments_only() {
        let found = segments(
            &answer(
                r#""intro": [{"start_ms": 60000, "end_ms": 150000, "submission_count": 4, "confidence": 0.9},
                             {"start_ms": 0, "end_ms": 30000, "submission_count": 2, "confidence": 0.9}],
                   "recap": [{"start_ms": null, "end_ms": 40000, "submission_count": 3, "confidence": 0.5}],
                   "credits": [{"start_ms": 2500000, "end_ms": 2599000, "submission_count": 5, "confidence": 0.8}]"#,
            ),
            LENGTH,
        );
        let kinds: Vec<_> = found.iter().map(|s| (s.kind.as_str(), s.start, s.end)).collect();
        assert_eq!(kinds, vec![("recap", 0., Some(40.)), ("intro", 60., Some(150.)), ("credits", 2500., None)]);
    }

    #[test]
    fn unverified_end_credits_bring_up_next_episode() {
        let found = segments(
            &answer(r#""credits": [{"start_ms": 2450000, "end_ms": null, "submission_count": 1, "confidence": 0.1}]"#),
            LENGTH,
        );
        assert_eq!(found, vec![Segment { kind: "end".into(), start: 2450., end: None }]);
        let early = segments(
            &answer(r#""credits": [{"start_ms": 900000, "end_ms": null, "submission_count": 1, "confidence": 0.1}]"#),
            LENGTH,
        );
        assert!(early.is_empty());
    }

    #[test]
    fn another_cut_or_garbage_gives_nothing() {
        assert!(segments(r#"{"group_duration_ms": 2000000}"#, LENGTH).is_empty());
        assert!(segments("not json", LENGTH).is_empty());
    }
}
