//! Pictures as Kotlin asks about them (`zcore::pictures`): where each place's picture comes from and at what size,
//! how a logo is toned, and the glow.

use zcore::api::BASE;
use zcore::pictures as p;

/// The sizes cards are cut at, "WxH" (`zcore::pictures`).
#[derive(Debug, uniffi::Record)]
pub struct PictureSizes {
    pub poster: String,
    pub landscape: String,
    pub thumb: String,
    /// The longest side a logo is decoded at for its tone, and for its hue.
    pub tone_side: u32,
    pub hue_side: u32,
}

#[uniffi::export]
pub fn picture_sizes() -> PictureSizes {
    PictureSizes {
        poster: p::POSTER.into(),
        landscape: p::LANDSCAPE.into(),
        thumb: p::THUMB.into(),
        tone_side: p::TONE_SIDE,
        hue_side: p::HUE_SIDE,
    }
}

/// A picture's whole address (`zcore::pictures::url`).
#[uniffi::export]
pub fn picture_url(url: String) -> String {
    p::url(BASE, &url)
}

/// A picture at a card's size (`zcore::pictures::card`).
#[uniffi::export]
pub fn picture_card(url: String, size: String) -> String {
    p::card(BASE, &url, &size)
}

/// The picture a big place shows: the biggest there is (`zcore::pictures::stage`).
#[uniffi::export]
pub fn picture_stage(cover_big: Option<String>, cover: Option<String>, poster: Option<String>) -> Option<String> {
    p::stage(cover_big.as_deref(), cover.as_deref(), poster.as_deref()).map(String::from)
}

/// The picture a landscape card or a small picture shows (`zcore::pictures::landscape`).
#[uniffi::export]
pub fn picture_landscape(cover: Option<String>, cover_big: Option<String>, poster: Option<String>) -> Option<String> {
    p::landscape(cover.as_deref(), cover_big.as_deref(), poster.as_deref()).map(String::from)
}

/// What a landscape card shows when its picture can't be had (`zcore::pictures::landscape_fallback`).
#[uniffi::export]
pub fn picture_landscape_fallback(cover: Option<String>, cover_big: Option<String>, poster: Option<String>) -> Option<String> {
    p::landscape_fallback(cover.as_deref(), cover_big.as_deref(), poster.as_deref()).map(String::from)
}

/// A title's poster at its sharpest (`zcore::pictures::poster`).
#[uniffi::export]
pub fn picture_poster(poster_hd: Option<String>, poster: Option<String>, cover: Option<String>, fallback: Option<String>) -> String {
    p::poster(BASE, poster_hd.as_deref(), poster.as_deref(), cover.as_deref(), fallback.as_deref())
}

/// Portrait art with no writing on it (`zcore::model::clean_art`).
#[uniffi::export]
pub fn picture_clean_art(poster_clean: bool, source: Option<String>, poster_hd: Option<String>) -> bool {
    zcore::model::clean_art(poster_clean, source.as_deref(), poster_hd.as_deref())
}

/// Clean art with a logo to carry over it (`zcore::model::own_art`).
#[uniffi::export]
pub fn picture_own_art(clean_art: bool, logo: Option<String>) -> bool {
    zcore::model::own_art(clean_art, logo.as_deref())
}

/// A drama: its card always carries its name (`zcore::pictures::drama`).
#[uniffi::export]
pub fn picture_drama(source: Option<String>) -> bool {
    p::drama(source.as_deref())
}

/// Whether a poster card writes the title's name under it (`zcore::pictures::poster_named`).
#[uniffi::export]
pub fn picture_poster_named(clean_art: bool, source: Option<String>) -> bool {
    p::poster_named(clean_art, source.as_deref())
}

/// How a logo is shown over the dark (`zcore::pictures::Tone`).
#[derive(Debug, uniffi::Enum)]
pub enum LogoTone {
    AsIs,
    Brighten { by: f32 },
    Lift { by: f32 },
}

/// A logo's tone from its straight RGBA pixels, decoded at most `tone_side` long (`zcore::pictures::tone`).
#[uniffi::export]
pub fn logo_tone(rgba: Vec<u8>) -> LogoTone {
    match p::tone(p::peak(&rgba)) {
        p::Tone::AsIs => LogoTone::AsIs,
        p::Tone::Brighten { by } => LogoTone::Brighten { by },
        p::Tone::Lift { by } => LogoTone::Lift { by },
    }
}

/// A coloured logo's hue in degrees from its straight RGBA pixels, decoded at most `hue_side` long; None when it's
/// white, grey or black (`zcore::pictures::logo_hue`).
#[uniffi::export]
pub fn logo_hue(rgba: Vec<u8>) -> Option<f32> {
    p::logo_hue(&rgba)
}

/// The colour a page glows in, ARGB, or None (`zcore::pictures::glow`).
#[uniffi::export]
pub fn glow_colour(logo_hue: Option<f32>, art_colour: Option<u32>) -> Option<u32> {
    p::glow(logo_hue, art_colour)
}

/// How art colours are asked for and kept (`zcore::pictures`).
#[derive(Debug, uniffi::Record)]
pub struct ArtColourRules {
    pub neutral: u32,
    pub most: u32,
    pub retry_ms: u32,
    pub at_once: u32,
}

#[uniffi::export]
pub fn art_colour_rules() -> ArtColourRules {
    ArtColourRules {
        neutral: p::NEUTRAL,
        most: u32::try_from(p::ART_COLOURS_MOST).unwrap_or(u32::MAX),
        retry_ms: p::ART_COLOUR_RETRY_MS,
        at_once: u32::try_from(p::ART_COLOURS_AT_ONCE).unwrap_or(u32::MAX),
    }
}

/// Where a picture's colour is asked for (`zcore::pictures::art_colour_path`).
#[uniffi::export]
pub fn art_colour_path(picture: String) -> String {
    p::art_colour_path(&picture)
}

/// A picture's colour from the server's answer, ARGB (`zcore::pictures::art_colour`).
#[uniffi::export]
pub fn art_colour(answer: String) -> u32 {
    p::art_colour(&answer)
}
