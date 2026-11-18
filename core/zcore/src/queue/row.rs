//! A download as `downloads.json` keeps it: one list for films, episodes and manhwa chapters, read as both apps have
//! ever written it (Android's leaves out what isn't there and keeps `length`; Windows' keeps `kind`, `done`, a
//! chapter's `bytes` and `thumb`, and "DELETING"), and written back with every field it had, ones this version doesn't
//! know included.

use crate::downloads::{self as rules, Kind, State};
use serde_json::{json, Map, Value};

/// Where the list is kept, in the app's files folder.
pub const LIST: &str = "downloads.json";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    /// A video's episode id (its title's for a film); a chapter's path.
    pub key: String,
    pub kind: Kind,
    /// A video's title; a chapter's series path.
    pub title_id: String,
    pub episode_id: Option<String>,
    /// The title's or series' name.
    pub title: String,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    /// An episode's still (a film's or a series' picture).
    pub cover: Option<String>,
    /// The title's own picture: a show's stacked entry shows it.
    pub art: Option<String>,
    pub imdb: Option<String>,
    /// Where a video comes from ([`rules::copy_of`]).
    pub copy: String,
    /// A video's level ("1080p"), "High" till it's known; a chapter's label ("Chapter 12").
    pub label: String,
    /// The quality a video was saved with ("720p", "1080p", "best").
    pub quality: String,
    /// A video's bytes all told (0 till known); a chapter's pages.
    pub size: u64,
    /// What's down: a video's bytes, a chapter's pages.
    pub done: u64,
    pub state: State,
    /// Deleted while its transfer let go of its files (Windows'): its files go as the app starts.
    pub deleting: bool,
    pub note: String,
    /// A video's length in seconds (0: not known).
    pub length: f64,
    /// The profiles that saved it; None: saved before profiles had their own, so everyone's.
    pub profiles: Option<Vec<String>>,
    /// A finished chapter's bytes on the device.
    pub bytes: u64,
    /// A chapter's still.
    pub thumb: Option<String>,
    /// Fields this version doesn't know, kept as they were.
    pub extra: Map<String, Value>,
}

const KNOWN: [&str; 21] = [
    "key", "kind", "titleId", "episodeId", "title", "season", "episode", "cover", "art", "imdb", "copy", "label", "quality",
    "size", "done", "state", "note", "length", "profiles", "bytes", "thumb",
];

fn text(o: &Map<String, Value>, k: &str) -> Option<String> {
    match o.get(k)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn number(o: &Map<String, Value>, k: &str) -> Option<f64> {
    match o.get(k)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn whole(o: &Map<String, Value>, k: &str) -> Option<i64> {
    number(o, k).map(|n| n as i64)
}

impl Row {
    /// One entry of the list; None for one that isn't an object or has no key.
    pub fn read(value: &Value) -> Option<Row> {
        let o = value.as_object()?;
        let key = text(o, "key").filter(|k| !k.is_empty())?;
        let state_text = text(o, "state").unwrap_or_default();
        let quality = text(o, "quality").unwrap_or_default();
        let extra = o.iter().filter(|(k, _)| !KNOWN.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
        Some(Row {
            kind: Kind::parse(&text(o, "kind").unwrap_or_default()),
            title_id: text(o, "titleId").unwrap_or_default(),
            episode_id: text(o, "episodeId").filter(|e| !e.is_empty()),
            title: text(o, "title").unwrap_or_default(),
            season: whole(o, "season"),
            episode: whole(o, "episode"),
            cover: text(o, "cover").filter(|c| !c.is_empty()),
            art: text(o, "art").filter(|c| !c.is_empty()),
            imdb: text(o, "imdb").filter(|c| !c.is_empty()),
            copy: text(o, "copy").unwrap_or_default(),
            label: rules::migrated_label(&text(o, "label").unwrap_or_default(), &quality),
            quality,
            size: number(o, "size").unwrap_or(0.).max(0.) as u64,
            done: number(o, "done").unwrap_or(0.).max(0.) as u64,
            deleting: state_text == "DELETING",
            state: State::parse(&state_text),
            note: text(o, "note").unwrap_or_default(),
            length: number(o, "length").unwrap_or(0.),
            profiles: o.get("profiles").and_then(Value::as_array).map(|l| l.iter().filter_map(|p| p.as_str().map(String::from)).collect()),
            bytes: number(o, "bytes").unwrap_or(0.).max(0.) as u64,
            thumb: text(o, "thumb").filter(|c| !c.is_empty()),
            extra,
            key,
        })
    }

    /// The entry as it's kept: nothing written for what isn't there.
    pub fn write(&self) -> Value {
        let mut o = self.extra.clone();
        let mut put = |k: &str, v: Value| {
            o.insert(k.into(), v);
        };
        put("key", json!(self.key));
        put("kind", json!(self.kind.as_str()));
        put("titleId", json!(self.title_id));
        if let Some(e) = &self.episode_id {
            put("episodeId", json!(e));
        }
        put("title", json!(self.title));
        if let Some(s) = self.season {
            put("season", json!(s));
        }
        if let Some(e) = self.episode {
            put("episode", json!(e));
        }
        for (k, v) in [("cover", &self.cover), ("art", &self.art), ("imdb", &self.imdb), ("thumb", &self.thumb)] {
            if let Some(v) = v {
                put(k, json!(v));
            }
        }
        if !self.copy.is_empty() {
            put("copy", json!(self.copy));
        }
        put("label", json!(self.label));
        if !self.quality.is_empty() {
            put("quality", json!(self.quality));
        }
        put("size", json!(self.size));
        put("done", json!(self.done));
        put("state", json!(if self.deleting { "DELETING" } else { self.state.as_str() }));
        put("note", json!(self.note));
        if self.length > 0. {
            put("length", json!(self.length));
        }
        if let Some(p) = &self.profiles {
            put("profiles", json!(p));
        }
        if self.bytes > 0 {
            put("bytes", json!(self.bytes));
        }
        Value::Object(o)
    }

    /// The name a row or a notification gives it: a film's title, "Title · E3".
    pub fn named(&self) -> String {
        match self.episode {
            Some(n) => format!("{} · E{n}", self.title),
            None => self.title.clone(),
        }
    }
}

/// The whole list, as kept: entries that can't be read are dropped.
pub fn read_list(text: &str) -> Vec<Row> {
    serde_json::from_str::<Value>(text).ok().and_then(|v| v.as_array().map(|a| a.iter().filter_map(Row::read).collect())).unwrap_or_default()
}

pub fn write_list(rows: &[Row]) -> String {
    Value::Array(rows.iter().map(Row::write).collect()).to_string()
}

/// A chapter waiting in Android's chapter queue from before chapters were in this list (`manhwa/queue-….json`).
pub fn read_chapter_queue(text: &str) -> Vec<Row> {
    let Ok(Value::Array(all)) = serde_json::from_str::<Value>(text) else { return vec![] };
    all.iter()
        .filter_map(|v| {
            let o = v.as_object()?;
            let path = text_of(o, "chapter")?;
            Some(Row {
                key: path,
                kind: Kind::Chapter,
                title_id: text_of(o, "series").unwrap_or_default(),
                title: text_of(o, "title").unwrap_or_default(),
                cover: text_of(o, "cover"),
                art: text_of(o, "cover"),
                label: text_of(o, "label").unwrap_or_default(),
                state: State::parse(&text_of(o, "state").unwrap_or_default()).restored(),
                note: text_of(o, "note").unwrap_or_default(),
                ..Default::default()
            })
        })
        .collect()
}

fn text_of(o: &Map<String, Value>, k: &str) -> Option<String> {
    text(o, k).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// downloads.json as the Android app wrote it (build 1244).
    const ANDROID: &str = r#"[{"key":"tt1-s1e2","titleId":"cinema-series-tt1","episodeId":"tt1-s1e2","title":"Show","season":1,"episode":2,"cover":"https://i/c.jpg","imdb":"tt1","copy":"dulo|cinema-series-tt1|tt1-s1e2","label":"High","quality":"1080p","size":1200000000,"state":"DONE","note":"","length":2700.0,"art":"https://i/a.jpg","profiles":["p1","p2"]},
        {"key":"anime-5","titleId":"anime-9","episodeId":"anime-5","title":"Anime","episode":5,"copy":"anime|5|9|sub","label":"Standard","quality":"720p","size":0,"state":"RUNNING","note":""}]"#;
    /// downloads.json as the Windows app wrote it.
    const WINDOWS: &str = r#"[{"profiles":["p1"],"kind":"video","key":"cinema-movie-tt9","titleId":"cinema-movie-tt9","episodeId":null,"title":"Film","season":null,"episode":null,"cover":"https://i/f.jpg","art":"https://i/f.jpg","imdb":"tt9","copy":"dulo|cinema-movie-tt9|","label":"4K","quality":"best","size":5000,"done":2500,"state":"DELETING","note":""},
        {"profiles":["p1"],"kind":"chapter","key":"/comics/a/chapter/3","titleId":"/comics/a","title":"Alpha","cover":"https://c/s.jpg","art":"https://c/l.jpg","label":"Chapter 3","thumb":"https://c/t.jpg","size":40,"done":40,"state":"DONE","note":"","bytes":12000000,"future":{"x":1}}]"#;

    #[test]
    fn both_apps_lists_are_read() {
        let rows = read_list(ANDROID);
        assert_eq!(rows.len(), 2);
        let show = &rows[0];
        assert_eq!((show.kind, show.state, show.size, show.length), (Kind::Video, State::Done, 1_200_000_000, 2700.));
        // (an older "High" says the level it got)
        assert_eq!(show.label, "1080p");
        assert_eq!(show.profiles.as_deref(), Some(&["p1".to_string(), "p2".to_string()][..]));
        assert_eq!(show.named(), "Show · E2");
        assert_eq!(rows[1].season, None);
        let rows = read_list(WINDOWS);
        assert!(rows[0].deleting && rows[0].episode_id.is_none());
        assert_eq!((rows[0].done, rows[0].label.as_str()), (2500, "4K"));
        let chapter = &rows[1];
        assert_eq!((chapter.kind, chapter.bytes, chapter.thumb.as_deref()), (Kind::Chapter, 12_000_000, Some("https://c/t.jpg")));
        assert_eq!(read_list("not json"), vec![]);
    }

    #[test]
    fn written_back_with_everything_it_had() {
        for list in [ANDROID, WINDOWS] {
            let rows = read_list(list);
            assert_eq!(read_list(&write_list(&rows)), rows);
        }
        let written = write_list(&read_list(WINDOWS));
        assert!(written.contains(r#""future":{"x":1}"#), "{written}");
        assert!(written.contains(r#""state":"DELETING""#));
        // (an Android row read back by an older Android: the fields it reads, as it wrote them)
        let back: Value = serde_json::from_str(&write_list(&read_list(ANDROID))).unwrap_or_default();
        assert_eq!(back[0]["copy"], "dulo|cinema-series-tt1|tt1-s1e2");
        assert_eq!(back[0]["season"], 1);
        assert_eq!(back[0]["state"], "DONE");
        assert!(back[1].get("season").is_none());
    }

    #[test]
    fn androids_chapter_queue_joins_the_list() {
        let queue = r#"[{"series":"/comics/a","title":"Alpha","cover":"https://c/a.jpg","chapter":"/comics/a/chapter/4","label":"Chapter 4","state":"RUNNING","note":""}]"#;
        let rows = read_chapter_queue(queue);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].kind, rows[0].state, rows[0].key.as_str()), (Kind::Chapter, State::Queued, "/comics/a/chapter/4"));
        assert_eq!(rows[0].title_id, "/comics/a");
    }
}
