//! What the apps know about pictures, kept and asked for as the core says (`zcore::art_answers`).

use std::sync::Mutex;
use zcore::art_answers as a;

/// What a finished answer needs done (`zcore::art_answers::Done`).
#[derive(Debug, uniffi::Record)]
pub struct ArtDone {
    pub start: Vec<String>,
    pub save: Option<String>,
    pub retry_ms: Option<u32>,
    pub changed: bool,
}

/// One store of answers: picture colours, or the writing on pictures and logos' tones.
#[derive(uniffi::Object)]
pub struct ArtAnswers {
    inner: Mutex<a::Answers>,
    file: &'static str,
}

impl ArtAnswers {
    fn lock(&self) -> std::sync::MutexGuard<'_, a::Answers> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[uniffi::export]
impl ArtAnswers {
    /// The picture colours' store.
    #[uniffi::constructor]
    pub fn colours() -> Self {
        Self { inner: Mutex::new(a::Answers::new(a::Kind::Colours)), file: a::Kind::Colours.file() }
    }

    /// The store of the writing on pictures, and of logos.
    #[uniffi::constructor]
    pub fn text() -> Self {
        Self { inner: Mutex::new(a::Answers::new(a::Kind::Text)), file: a::Kind::Text.file() }
    }

    /// The file in the app's folder the answers are kept in.
    pub fn file(&self) -> String {
        self.file.to_string()
    }

    /// Takes in the kept file's text.
    pub fn restore(&self, saved: String) {
        self.lock().restore(&saved);
    }

    pub fn get(&self, key: String) -> Option<i64> {
        self.lock().get(&key)
    }

    /// The keys to start asking for now.
    pub fn want(&self, key: String) -> Vec<String> {
        self.lock().want(&key)
    }

    /// A started key is answered (None: it could not be).
    pub fn done(&self, key: String, answer: Option<i64>) -> ArtDone {
        let d = self.lock().done(&key, answer);
        ArtDone { start: d.start, save: d.save, retry_ms: d.retry_ms, changed: d.changed }
    }

    /// The time to ask again has come for a key.
    pub fn retry(&self, key: String) {
        self.lock().retry(&key);
    }
}

#[uniffi::export]
pub fn art_text_key(url: String) -> String {
    a::text_key(&url)
}

#[uniffi::export]
pub fn art_logo_key(url: String) -> String {
    a::logo_key(&url)
}

/// The kept answer about the writing in the server's answer (None: it can't be read).
#[uniffi::export]
pub fn art_writing_code(answer: String, title: String) -> Option<i64> {
    zcore::art::writing_code(&answer, &title)
}

/// The kept answer about a logo's script (`zcore::art::latin_code`).
#[uniffi::export]
pub fn art_latin_code(answer: Option<String>, refused: bool) -> Option<i64> {
    zcore::art::latin_code(answer.as_deref(), refused)
}

/// The writing a kept number stands for.
#[uniffi::export]
pub fn art_writing_of_code(code: i64) -> crate::Writing {
    zcore::art::Writing::from_code(code).into()
}

/// What the app knows of the writing on pictures, asked for when the core wants it (`zcore::art::Looks`).
#[uniffi::export(with_foreign)]
pub trait ArtLooks: Send + Sync {
    /// The writing on a backdrop of `title`; None until known.
    fn writing(&self, picture: String, title: String) -> Option<crate::Writing>;
    /// Whether a logo is in a script the viewers read; None until known.
    fn latin(&self, logo: String) -> Option<bool>;
}

struct Looks(std::sync::Arc<dyn ArtLooks>);

impl zcore::art::Looks for Looks {
    fn writing(&self, picture: &str, title: &str) -> Option<zcore::art::Writing> {
        self.0.writing(picture.into(), title.into()).map(Into::into)
    }

    fn latin(&self, logo: &str) -> Option<bool> {
        self.0.latin(logo.into())
    }
}

/// What a landscape card needs of a title to choose its art (`zcore::art::CardFacts`).
#[derive(Debug, uniffi::Record)]
pub struct CardFacts {
    pub source: Option<String>,
    pub textless: bool,
    pub cover: Option<String>,
    pub cover_big: Option<String>,
    pub poster: Option<String>,
    pub logo: Option<String>,
    pub title: String,
}

/// How a landscape card shows a title (`zcore::art::CardArt`).
#[derive(Debug, uniffi::Record)]
pub struct CardArt {
    pub cover: Option<String>,
    pub logo: Option<String>,
    pub name: bool,
}

/// A landscape card's art, whole (`zcore::art::card_art`).
#[uniffi::export]
pub fn art_card(facts: CardFacts, looks: std::sync::Arc<dyn ArtLooks>) -> CardArt {
    let f = zcore::art::CardFacts {
        source: facts.source.as_deref(),
        textless: facts.textless,
        cover: facts.cover.as_deref(),
        cover_big: facts.cover_big.as_deref(),
        poster: facts.poster.as_deref(),
        logo: facts.logo.as_deref(),
        title: &facts.title,
    };
    let a = zcore::art::card_art(&f, &Looks(looks));
    CardArt { cover: a.cover, logo: a.logo, name: a.name }
}
