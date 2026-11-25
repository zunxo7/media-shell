//! Pictures, the same on every app: which of a title's pictures each place shows, at what size and from where, how a
//! logo is toned to read on the dark, and the colour a page glows in. The apps decode pictures and hand the pixels
//! over; what they mean is decided here.

/// A card's size as the server's resizer cuts it, "WxH".
pub const POSTER: &str = "268x402";
/// A landscape card.
pub const LANDSCAPE: &str = "640x360";
/// The stage's small pictures of the titles to come, and a title's episode stills.
pub const THUMB: &str = "440x248";

/// Hosts the server's resizer takes (`/card`): their pictures are shrunk once there, not sent whole to every device.
const RESIZED: [&str; 8] = [
    "images.aryplus.tv",
    "m.media-amazon.com",
    "i.ytimg.com",
    "s4.anilist.co",
    "cdn.myanimelist.net",
    "myanimelist.net",
    "animehot.cc",
    "hianime.at",
];
/// ARY's own folders, sent as bare paths on its picture host.
const ARY_FOLDERS: [&str; 5] = ["mobile", "logo", "poster", "big", "desktop"];

fn is_http(url: &str) -> bool {
    url.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("http"))
}

/// A path's characters as a URL keeps them; anything else percent-encoded.
fn path_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/:?=&,-_.!~*'()#+$;@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A picture's whole address: ARY sends paths on its own picture host, the server its own paths; metahub's posters
/// are asked for at "medium".
pub fn url(base: &str, url: &str) -> String {
    if url.is_empty() {
        return String::new();
    }
    let full = if is_http(url) {
        url.to_string()
    } else if let Some(rest) = url.strip_prefix("//") {
        format!("https://{rest}")
    } else if url.starts_with('/') {
        format!("{base}{url}")
    } else {
        format!("https://images.aryplus.tv/{}", path_encode(url))
    };
    full.replace("images.metahub.space/poster/small/", "images.metahub.space/poster/medium/")
}

/// The scheme, host and path of an address.
fn parts(url: &str) -> Option<(&str, &str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = rest[..end].rsplit('@').next()?.split(':').next()?;
    let path = rest[end..].split(['?', '#']).next().unwrap_or_default();
    Some((scheme, host, path))
}

/// A picture at a card's `size` ("WxH"): TMDB cuts its own (asked for straight from it, at the cut nearest the
/// card's width); the hosts the server's resizer takes are shrunk once there; anything else as it is.
pub fn card(base: &str, picture: &str, size: &str) -> String {
    if picture.is_empty() {
        return String::new();
    }
    if !is_http(picture) && !picture.starts_with('/') && !picture.starts_with("//") {
        let folder = picture.split_once('/').map(|(f, _)| f);
        return if folder.is_some_and(|f| ARY_FOLDERS.contains(&f)) {
            format!("{base}/card/{size}/{}", path_encode(picture))
        } else {
            url(base, picture)
        };
    }
    let full = url(base, picture);
    let Some((scheme, host, path)) = parts(&full) else { return full };
    if host == "image.tmdb.org" {
        if let Some(at) = full.find("/t/p/") {
            let width: u32 = size.split('x').next().and_then(|w| w.parse().ok()).unwrap_or(342);
            let cut = if width <= 342 { "w342" } else if width <= 780 { "w780" } else { "w1280" };
            if let Some(slash) = full[at + 5..].find('/') {
                return format!("{}/t/p/{cut}{}", &full[..at], &full[at + 5 + slash..]);
            }
        }
        return full;
    }
    let ours = base.split_once("://").map(|(_, h)| h.trim_end_matches('/')).unwrap_or_default();
    let resized = scheme == "https"
        && (host.ends_with(".metahub.space")
            || RESIZED.contains(&host)
            || (host == ours && path.starts_with("/still/") && path.ends_with(".jpg")));
    if resized {
        format!("{base}/card/{size}?u={}", crate::sections::encode(&full))
    } else {
        full
    }
}

fn first(choices: [Option<&str>; 3]) -> Option<&str> {
    choices.into_iter().flatten().find(|s| !s.is_empty())
}

/// The picture a big place shows (the stage, a title's backdrop, the glow's art): the biggest there is.
pub fn stage<'a>(cover_big: Option<&'a str>, cover: Option<&'a str>, poster: Option<&'a str>) -> Option<&'a str> {
    first([cover_big, cover, poster])
}

/// The picture a landscape card or a small picture shows: the cover cut for it, else the biggest, else the poster.
pub fn landscape<'a>(cover: Option<&'a str>, cover_big: Option<&'a str>, poster: Option<&'a str>) -> Option<&'a str> {
    first([cover, cover_big, poster])
}

/// What a landscape card shows when its picture can't be had: the big cover when it's another picture, else the
/// poster.
pub fn landscape_fallback<'a>(cover: Option<&'a str>, cover_big: Option<&'a str>, poster: Option<&'a str>) -> Option<&'a str> {
    first([cover_big.filter(|b| Some(*b) != cover), poster, None])
}

/// A title's poster at its sharpest (metahub's large, ARY's big one): shrunk to the card, never stretched.
pub fn poster(base: &str, poster_hd: Option<&str>, poster: Option<&str>, cover: Option<&str>, fallback: Option<&str>) -> String {
    let p = [poster_hd, poster, cover, fallback].into_iter().flatten().find(|s| !s.is_empty()).unwrap_or_default();
    url(base, &p.replace("/poster/small/", "/poster/large/").replace("/poster/medium/", "/poster/large/"))
}

/// A drama (ARY's, or one from YouTube's channels): its card always carries its name, whatever its picture has on it.
pub fn drama(source: Option<&str>) -> bool {
    matches!(source, None | Some("ary") | Some("youtube"))
}

/// Whether a poster card writes the title's name under it (and shades its foot for it): its art is clean, or it's a
/// drama. A logo, when it has one, goes there instead.
pub fn poster_named(clean_art: bool, source: Option<&str>) -> bool {
    clean_art || drama(source)
}

// ---------- logos ----------

/// The longest side a logo is judged at for its tone (the apps decode it this small).
pub const TONE_SIDE: u32 = 192;
/// The longest side a logo is judged at for its hue.
pub const HUE_SIDE: u32 = 120;
/// A pixel this opaque (of 255) is part of the logo.
const SOLID: u8 = 140;
/// Fewer solid pixels than this: too little to say.
const TOO_FEW: usize = 20;

/// How a logo is shown over the dark so it always reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tone {
    /// Bright enough as it is.
    AsIs,
    /// A dark one (a deep teal), brightened by this much, its colour kept.
    Brighten { by: f32 },
    /// A black one, lifted towards white: each channel `c * (1 - by) + 255 * by`.
    Lift { by: f32 },
}

/// The solid pixels of straight (not premultiplied) RGBA, as (r, g, b) from 0 to 1.
fn solid(rgba: &[u8]) -> impl Iterator<Item = (f32, f32, f32)> + '_ {
    let (pixels, _) = rgba.as_chunks::<4>();
    pixels.iter().filter(|p| p[3] > SOLID).map(|p| (f32::from(p[0]) / 255., f32::from(p[1]) / 255., f32::from(p[2]) / 255.))
}

/// How lit a logo's brightest tenth is (its solid pixels' strongest channel), from its straight RGBA pixels; None
/// when it's too small to say.
pub fn peak(rgba: &[u8]) -> Option<f32> {
    let mut lit: Vec<f32> = solid(rgba).map(|(r, g, b)| r.max(g).max(b)).collect();
    if lit.len() < TOO_FEW {
        return None;
    }
    lit.sort_by(f32::total_cmp);
    lit.get(lit.len() * 9 / 10).copied()
}

/// The tone for a logo whose brightest tenth is `peak`: .6 and up as it is; .15 to .6 brightened up to .85 (at most
/// 4×); darker still lifted towards white. Unknown, as it is.
pub fn tone(peak: Option<f32>) -> Tone {
    match peak {
        Some(p) if p < 0.15 => Tone::Lift { by: 0.85 },
        Some(p) if p < 0.6 => Tone::Brighten { by: (0.85 / p).min(4.) },
        _ => Tone::AsIs,
    }
}

/// The number a logo's peak is kept as: in hundredths, 100 when there is too little to say.
pub fn tone_code(peak: Option<f32>) -> i64 {
    (peak.unwrap_or(1.) * 100.).round().clamp(0., 100.) as i64
}

/// The tone a kept number stands for ([`tone_code`]).
pub fn tone_of_code(code: i64) -> Tone {
    tone(Some(code.clamp(0, 100) as f32 / 100.))
}

/// A colour's hue in degrees, its saturation and value (0 to 1).
fn hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    let s = if max > 0. { d / max } else { 0. };
    let h = if d == 0. {
        0.
    } else if max == r {
        ((g - b) / d).rem_euclid(6.) * 60.
    } else if max == g {
        ((b - r) / d + 2.) * 60.
    } else {
        ((r - g) / d + 4.) * 60.
    };
    (h, s, max)
}

/// A coloured logo's hue in degrees (its solid, lit, saturated pixels, weighted by how vivid), from its straight RGBA
/// pixels; None when it's white, grey or black, or too small to say: the glow then takes the artwork's colour.
pub fn logo_hue(rgba: &[u8]) -> Option<f32> {
    let (mut x, mut y, mut weight, mut count) = (0f64, 0f64, 0f64, 0usize);
    for (r, g, b) in solid(rgba) {
        count += 1;
        let (h, s, v) = hsv(r, g, b);
        if v < 0.2 || s < 0.25 {
            continue;
        }
        let k = f64::from(s * v);
        let a = f64::from(h).to_radians();
        x += k * a.cos();
        y += k * a.sin();
        weight += k;
    }
    if count < TOO_FEW || weight / (count as f64) < 0.12 {
        return None;
    }
    Some((y.atan2(x).to_degrees().rem_euclid(360.)) as f32)
}

/// How long after a slide comes in the next one's picture is read ahead, ms: the slide showing gets the line first.
pub const WARM_AFTER_MS: u32 = 900;

// ---------- the glow ----------

/// The colour colourless art comes back as from the server (`/api/artcolor`), ARGB.
pub const NEUTRAL: u32 = 0xFF3A3A3A;
/// The most art colours an app keeps; past that it starts again.
pub const ART_COLOURS_MOST: usize = 5000;
/// How long before a colour the server couldn't give is asked for again, ms.
pub const ART_COLOUR_RETRY_MS: u32 = 20_000;
/// How many art colours are asked for at once.
pub const ART_COLOURS_AT_ONCE: usize = 6;

/// Where a picture's colour is asked for.
pub fn art_colour_path(picture: &str) -> String {
    format!("/api/artcolor?u={}", crate::sections::encode(picture))
}

/// A picture's colour from the server's answer, ARGB; colourless art (or an answer with none) is [`NEUTRAL`].
pub fn art_colour(answer: &str) -> u32 {
    serde_json::from_str::<serde_json::Value>(answer)
        .ok()
        .and_then(|v| v.get("color")?.as_str().map(String::from))
        .and_then(|hex| u32::from_str_radix(hex.trim_start_matches('#'), 16).ok())
        .map_or(NEUTRAL, |rgb| 0xFF00_0000 | (rgb & 0xFF_FFFF))
}

/// A colour's hue, None when it's too grey to have one.
fn hue_of(argb: u32) -> Option<f32> {
    let channel = |shift: u32| ((argb >> shift) & 255) as f32 / 255.;
    let (r, g, b) = (channel(16), channel(8), channel(0));
    if r.max(g).max(b) - r.min(g).min(b) < 0.08 {
        return None;
    }
    Some(hsv(r, g, b).0)
}

/// The pictures that light a page: its logo (the colour of a coloured one), and its artwork (the colour it falls back
/// on); paths, as the title's own. Shells fetch and decode them; the core says which, and how strongly (below).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GlowArt {
    pub logo: Option<String>,
    pub art: Option<String>,
}

impl GlowArt {
    /// The glow of a title with these pictures: its logo, and its stage picture ([`stage`]).
    pub fn of(logo: Option<&str>, cover_big: Option<&str>, cover: Option<&str>, poster: Option<&str>) -> Self {
        Self {
            logo: logo.filter(|l| !l.is_empty()).map(String::from),
            art: stage(cover_big, cover, poster).map(String::from),
        }
    }
}

/// A logo's hue kept as a byte: 0-254 round the circle, [`HUE_NONE`] when it has none (so the answers kept on a
/// device stay readable).
pub const HUE_NONE: u8 = 255;

/// A logo's hue ([`logo_hue`]) as its byte.
pub fn hue_code(hue: Option<f32>) -> u8 {
    hue.map_or(HUE_NONE, |deg| ((deg / 360.) * 254.).round().min(254.) as u8)
}

/// A hue byte ([`hue_code`]) back as degrees; None for [`HUE_NONE`].
pub fn hue_degrees(code: u8) -> Option<f32> {
    (code < HUE_NONE).then(|| f32::from(code) / 254. * 360.)
}

/// The colour a page glows in, from its logo's hue byte (read: [`hue_code`]) and, when the logo has none, its art's
/// colour (ask for it only then); None while there's nothing to light with.
pub fn glow_from_code(logo: u8, art_colour: impl FnOnce() -> Option<u32>) -> Option<u32> {
    match hue_degrees(logo) {
        Some(hue) => glow(Some(hue), None),
        None => glow(None, art_colour()),
    }
}

/// How the glow behind Home and a title's page eases off as the page scrolls: it keeps this much of its strength.
pub const GLOW_FLOOR: f32 = 0.35;
/// ...going over this much of the screen's height.
pub const GLOW_REACH: f32 = 0.7;

/// The glow's strength, 1 at the top of the page, with the page scrolled `scrolled` on a screen `screen` tall.
pub fn glow_strength(scrolled: f32, screen: f32) -> f32 {
    (1. - scrolled / (screen * GLOW_REACH).max(1.)).clamp(GLOW_FLOOR, 1.)
}

/// A title's picture behind its page dims as the page scrolls: down to this much of itself...
pub const ART_FLOOR: f32 = 0.15;
/// ...by the time it has scrolled this much of the way over its `band`'s height.
pub const ART_FADE: f32 = 0.85;

/// The picture's strength, 1 at the top, with the page scrolled `scrolled` under a picture `band` tall.
pub fn art_strength(scrolled: f32, band: f32) -> f32 {
    (1. - scrolled / band.max(1.) * ART_FADE).clamp(ART_FLOOR, 1.)
}

/// The colour a page glows in for a title, ARGB: its logo's hue when the logo is coloured (`logo_hue`), else its
/// artwork's colour's; None for colourless art. One brightness and depth for every title (HSV s .62, v .72): its
/// colour's character, never a glare.
pub fn glow(logo_hue: Option<f32>, art_colour: Option<u32>) -> Option<u32> {
    let hue = logo_hue.or_else(|| art_colour.filter(|c| *c != NEUTRAL).and_then(hue_of))?;
    let (s, v) = (0.62f32, 0.72f32);
    let c = v * s;
    let h = hue.rem_euclid(360.) / 60.;
    let x = c * (1. - (h.rem_euclid(2.) - 1.).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.),
        1 => (x, c, 0.),
        2 => (0., c, x),
        3 => (0., x, c),
        4 => (x, 0., c),
        _ => (c, 0., x),
    };
    let m = v - c;
    let to = |f: f32| (((f + m) * 255.).round() as u32).min(255);
    Some(0xFF00_0000 | to(r) << 16 | to(g) << 8 | to(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://host.invalid";

    #[test]
    fn addresses() {
        assert_eq!(url(BASE, "/still/a.jpg"), "https://host.invalid/still/a.jpg");
        assert_eq!(url(BASE, "//cdn.x/a.jpg"), "https://cdn.x/a.jpg");
        assert_eq!(url(BASE, "poster/a b.jpg"), "https://images.aryplus.tv/poster/a%20b.jpg");
        assert_eq!(
            url(BASE, "https://images.metahub.space/poster/small/tt1/img"),
            "https://images.metahub.space/poster/medium/tt1/img"
        );
        assert_eq!(url(BASE, ""), "");
    }

    #[test]
    fn cards_are_cut_where_they_can_be() {
        assert_eq!(card(BASE, "poster/a.jpg", POSTER), "https://host.invalid/card/268x402/poster/a.jpg");
        assert_eq!(card(BASE, "other/a.jpg", POSTER), "https://images.aryplus.tv/other/a.jpg");
        assert_eq!(
            card(BASE, "https://image.tmdb.org/t/p/original/x.jpg", LANDSCAPE),
            "https://image.tmdb.org/t/p/w780/x.jpg"
        );
        assert_eq!(card(BASE, "https://image.tmdb.org/t/p/original/x.jpg", POSTER), "https://image.tmdb.org/t/p/w342/x.jpg");
        assert_eq!(
            card(BASE, "https://i.ytimg.com/vi/a/hq.jpg", THUMB),
            "https://host.invalid/card/440x248?u=https%3A%2F%2Fi.ytimg.com%2Fvi%2Fa%2Fhq.jpg"
        );
        assert!(card(BASE, "https://host.invalid/still/s.jpg", THUMB).starts_with("https://host.invalid/card/440x248?u="));
        assert_eq!(card(BASE, "https://host.invalid/other.png", THUMB), "https://host.invalid/other.png");
        assert_eq!(card(BASE, "http://i.ytimg.com/a.jpg", THUMB), "http://i.ytimg.com/a.jpg");
        assert_eq!(card(BASE, "https://evil.com/i.ytimg.com/a.jpg", THUMB), "https://evil.com/i.ytimg.com/a.jpg");
    }

    #[test]
    fn each_place_has_its_order() {
        assert_eq!(stage(Some("big"), Some("c"), Some("p")), Some("big"));
        assert_eq!(stage(Some(""), Some("c"), Some("p")), Some("c"));
        assert_eq!(landscape(Some("c"), Some("big"), Some("p")), Some("c"));
        assert_eq!(landscape(None, None, Some("p")), Some("p"));
        assert_eq!(landscape_fallback(Some("c"), Some("big"), Some("p")), Some("big"));
        assert_eq!(landscape_fallback(Some("c"), Some("c"), Some("p")), Some("p"));
        assert_eq!(
            poster(BASE, None, Some("https://images.metahub.space/poster/small/tt1/img"), None, None),
            "https://images.metahub.space/poster/large/tt1/img"
        );
        assert!(poster_named(false, None) && poster_named(false, Some("youtube")) && poster_named(true, Some("cinema")));
        assert!(!poster_named(false, Some("anime")));
    }

    fn pixels(colour: [u8; 4], n: usize) -> Vec<u8> {
        colour.repeat(n)
    }

    #[test]
    fn logos_are_toned_to_read() {
        assert_eq!(tone(peak(&pixels([255, 255, 255, 255], 100))), Tone::AsIs);
        assert_eq!(tone(peak(&pixels([0, 0, 0, 255], 100))), Tone::Lift { by: 0.85 });
        let Tone::Brighten { by } = tone(peak(&pixels([0, 76, 76, 255], 100))) else { panic!("not brightened") };
        assert!((by - 0.85 / (76. / 255.)).abs() < 0.01);
        // (too few solid pixels to say, or see-through ones: as it is)
        assert_eq!(peak(&pixels([0, 0, 0, 255], 10)), None);
        assert_eq!(peak(&pixels([0, 0, 0, 100], 100)), None);
        assert_eq!(tone(None), Tone::AsIs);
    }

    #[test]
    fn a_coloured_logo_has_a_hue_and_a_white_one_none() {
        let red = logo_hue(&pixels([230, 20, 20, 255], 100)).unwrap_or(-1.);
        assert!(red < 1. || red > 359., "{red}");
        let blue = logo_hue(&pixels([20, 40, 230, 255], 100)).unwrap_or(-1.);
        assert!((blue - 234.).abs() < 2., "{blue}");
        assert_eq!(logo_hue(&pixels([255, 255, 255, 255], 100)), None);
        assert_eq!(logo_hue(&pixels([230, 20, 20, 255], 5)), None);
    }

    #[test]
    fn hues_keep_as_bytes_and_the_glow_reads_them() {
        assert_eq!(hue_code(None), HUE_NONE);
        assert_eq!(hue_code(Some(0.)), 0);
        assert_eq!(hue_code(Some(359.9)), 254);
        assert_eq!(hue_degrees(HUE_NONE), None);
        assert_eq!(hue_degrees(0), Some(0.));
        assert_eq!(glow_from_code(0, || panic!("the logo has a colour")), Some(0xFFB84646));
        assert_eq!(glow_from_code(HUE_NONE, || Some(0xFF2050E0)), glow(None, Some(0xFF2050E0)));
        assert_eq!(glow_from_code(HUE_NONE, || None), None);
    }

    #[test]
    fn the_glow_and_the_picture_ease_off_as_the_page_scrolls() {
        assert_eq!(glow_strength(0., 1000.), 1.);
        assert_eq!(glow_strength(350., 1000.), 0.5);
        assert_eq!(glow_strength(5000., 1000.), GLOW_FLOOR);
        assert_eq!(art_strength(0., 500.), 1.);
        assert_eq!(art_strength(5000., 500.), ART_FLOOR);
    }

    #[test]
    fn what_lights_a_page_is_its_logo_and_its_stage_picture() {
        let lit = GlowArt::of(Some(""), Some("big"), Some("c"), Some("p"));
        assert_eq!(lit, GlowArt { logo: None, art: Some("big".into()) });
        assert_eq!(GlowArt::of(Some("l"), None, None, Some("p")).logo.as_deref(), Some("l"));
    }

    #[test]
    fn the_glow_takes_the_logo_then_the_art() {
        assert_eq!(glow(Some(0.), None), Some(0xFFB84646));
        assert_eq!(glow(None, Some(0xFF2050E0)), glow(Some(hue_of(0xFF2050E0).unwrap_or_default()), None));
        assert_eq!(glow(None, Some(NEUTRAL)), None);
        assert_eq!(glow(None, Some(0xFF808080)), None);
        assert_eq!(glow(None, None), None);
        assert_eq!(art_colour(r##"{"color": "#2050e0"}"##), 0xFF2050E0);
        assert_eq!(art_colour("{}"), NEUTRAL);
        assert_eq!(art_colour_path("https://a.b/c d"), "/api/artcolor?u=https%3A%2F%2Fa.b%2Fc+d");
    }
}
