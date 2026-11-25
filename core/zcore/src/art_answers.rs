//! What the apps have worked out about pictures (a picture's colour, the writing on it, a logo's tone), kept on the
//! device for good: what is known, what to ask for and when, how many at once, when to ask again, what to forget, and
//! the kept file's format. The apps only do the asking (the server's answer, or decoding the picture) and the
//! writing of the file; they ask here what to do next.

use std::collections::{HashMap, HashSet, VecDeque};

/// How many answers about writing and tones are worked out at once.
pub const TEXT_AT_ONCE: usize = 4;

/// Which answers a store keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Picture colours (`art-colours.json`, keyed by the picture's address, the value an ARGB number): retried after
    /// a while, and started again past a few thousand.
    Colours,
    /// Writing, logo scripts and logo tones (`art-text.json`, keyed by [`text_key`] and the like): never retried
    /// until the app opens again, and never forgotten.
    Text,
}

impl Kind {
    /// The file in the app's own folder the answers are kept in.
    pub fn file(self) -> &'static str {
        match self {
            Kind::Colours => "art-colours.json",
            Kind::Text => "art-text.json",
        }
    }
}

/// The key for the writing on a picture.
pub fn text_key(url: &str) -> String {
    format!("text:{url}")
}
/// The key for whether a logo is in a script the viewers read.
pub fn logo_key(url: &str) -> String {
    format!("logo2:{url}")
}
/// The key for a logo's tone.
pub fn tone_key(url: &str) -> String {
    format!("tone:{url}")
}

/// The key for a logo's own colour, as a hue (0 to 254 of the circle, 255 for none).
pub fn hue_key(url: &str) -> String {
    format!("hue1:{url}")
}

/// What a finished answer needs done.
#[derive(Debug, Default, PartialEq)]
pub struct Done {
    /// Keys to start asking for now (the ones that were waiting for a turn).
    pub start: Vec<String>,
    /// The whole file to keep now (the answers changed).
    pub save: Option<String>,
    /// After this long, call [`Answers::retry`] for the key (it could not be answered, and will be asked again).
    pub retry_ms: Option<u32>,
    /// The answer is new: what is drawn from it looks again.
    pub changed: bool,
}

/// One store of answers.
pub struct Answers {
    kind: Kind,
    known: HashMap<String, i64>,
    asked: HashSet<String>,
    waiting: VecDeque<String>,
    running: usize,
}

impl Answers {
    pub fn new(kind: Kind) -> Self {
        Self { kind, known: HashMap::new(), asked: HashSet::new(), waiting: VecDeque::new(), running: 0 }
    }

    fn at_once(&self) -> usize {
        match self.kind {
            Kind::Colours => crate::pictures::ART_COLOURS_AT_ONCE,
            Kind::Text => TEXT_AT_ONCE,
        }
    }

    /// Takes in the kept file's text. What can't be read is as if it was not there; answers of an old kind (`logo:`,
    /// before logos were judged as they are now) are dropped, and so asked again once.
    pub fn restore(&mut self, saved: &str) {
        let Ok(map) = serde_json::from_str::<HashMap<String, serde_json::Value>>(saved) else { return };
        for (key, value) in map {
            if self.kind == Kind::Text && key.starts_with("logo:") {
                continue;
            }
            if let Some(n) = value.as_i64() {
                self.known.entry(key).or_insert(n);
            }
        }
    }

    /// The answer, if known.
    pub fn get(&self, key: &str) -> Option<i64> {
        self.known.get(key).copied()
    }

    /// The answer is wanted: the keys to start asking for now (this one, unless it is known, already asked for or has
    /// to wait its turn).
    pub fn want(&mut self, key: &str) -> Vec<String> {
        if key.is_empty() || self.known.contains_key(key) || !self.asked.insert(key.to_string()) {
            return Vec::new();
        }
        self.waiting.push_back(key.to_string());
        self.next()
    }

    /// Whether a key is waiting for its turn to be asked.
    pub fn is_waiting(&self, key: &str) -> bool {
        self.waiting.iter().any(|k| k == key)
    }

    fn next(&mut self) -> Vec<String> {
        let mut start = Vec::new();
        while self.running < self.at_once() {
            let Some(key) = self.waiting.pop_front() else { break };
            self.running += 1;
            start.push(key);
        }
        start
    }

    /// A started key is answered (`None`: it could not be).
    pub fn done(&mut self, key: &str, answer: Option<i64>) -> Done {
        self.running = self.running.saturating_sub(1);
        let mut done = Done::default();
        match answer {
            Some(value) => {
                if self.kind == Kind::Colours && self.known.len() >= crate::pictures::ART_COLOURS_MOST {
                    self.known.clear();
                }
                self.known.insert(key.to_string(), value);
                done.changed = true;
                done.save = serde_json::to_string(&self.known).ok();
            }
            None if self.kind == Kind::Colours => done.retry_ms = Some(crate::pictures::ART_COLOUR_RETRY_MS),
            None => {}
        }
        done.start = self.next();
        done
    }

    /// The time to ask again has come for a key that could not be answered: it is asked for when wanted next.
    pub fn retry(&mut self, key: &str) {
        self.asked.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_each_once_a_few_at_once() {
        let mut a = Answers::new(Kind::Text);
        let keys: Vec<String> = (0..6).map(|i| format!("text:{i}")).collect();
        let started: Vec<String> = keys.iter().flat_map(|k| a.want(k)).collect();
        assert_eq!(started, keys[..TEXT_AT_ONCE].to_vec());
        assert!(a.want(&keys[0]).is_empty() && a.want(&keys[5]).is_empty());
        let done = a.done(&keys[0], Some(2));
        assert_eq!((done.start, done.changed), (vec![keys[4].clone()], true));
        assert_eq!(a.get(&keys[0]), Some(2));
        assert!(a.want(&keys[0]).is_empty());
        let done = a.done(&keys[1], None);
        assert_eq!((done.start, done.save, done.retry_ms, done.changed), (vec![keys[5].clone()], None, None, false));
        assert!(a.want(&keys[1]).is_empty(), "not asked again until the app opens");
    }

    #[test]
    fn colours_are_retried_and_started_again() {
        let mut a = Answers::new(Kind::Colours);
        assert_eq!(a.want("u"), vec!["u".to_string()]);
        let done = a.done("u", None);
        assert_eq!(done.retry_ms, Some(20_000));
        assert!(a.want("u").is_empty());
        a.retry("u");
        assert_eq!(a.want("u"), vec!["u".to_string()]);
        for i in 0..crate::pictures::ART_COLOURS_MOST {
            a.known.insert(format!("k{i}"), 1);
        }
        let done = a.done("u", Some(0xFF11_2233));
        assert_eq!(a.get("k1"), None);
        assert_eq!(a.get("u"), Some(0xFF11_2233));
        assert_eq!(done.save.as_deref(), Some(r#"{"u":4279312947}"#));
    }

    #[test]
    fn reads_files_kept_before() {
        let mut c = Answers::new(Kind::Colours);
        c.restore(r#"{"https://x/a.jpg": 4279312947, "bad": "x"}"#);
        assert_eq!((c.get("https://x/a.jpg"), c.get("bad")), (Some(0xFF11_2233), None));
        let mut t = Answers::new(Kind::Text);
        t.restore(r#"{"text:a": 3, "logo2:a": 1, "tone:a": 55, "logo:a": 0}"#);
        assert_eq!((t.get("text:a"), t.get("logo2:a"), t.get("tone:a"), t.get("logo:a")), (Some(3), Some(1), Some(55), None));
        t.restore("not json");
        assert_eq!(t.get("text:a"), Some(3));
    }
}
