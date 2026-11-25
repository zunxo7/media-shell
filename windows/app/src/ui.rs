//! The phone app's pieces (design/SPEC.md), built from the design tokens: every size, colour and curve here comes from
//! `tokens`, never typed in. Screens are made of these, so they look and move as the phone's do.
use crate::data::{Item, Me};
use crate::gfx::{Font, TextStyle};
use crate::kit::{col, el, icon, k, row, text, Cb, El, FieldSpec, Fill};
use crate::tokens as t;
use crate::{net, App};

// ---------- colours ----------
/// White at this opacity (the tokens' w-scale: "w12" is `w(t::W12)`).
/// The page: near-black, not pure black, so the cinema pattern can sit on it.
pub const PAGE: u32 = t::C_PAGE;

/// Behind every page: a soft light from the upper left fading into deep charcoal, baked into a picture with a fine
/// dither so its shades blend instead of banding (as a gradient drawn live did).
/// `left`: how far from the window's left edge the page starts: the picture is laid as across the whole window, so a
/// page beside the rail and the rail itself show one gradient, no seam where they meet.
pub fn page_ground(key: &str, left: f32) -> El {
    el().key(key).at(-left, 0., 0., 0.).decorative().image("res:backdrop").backdrop(0.3, 0)
}

pub fn w(a: f32) -> u32 {
    ((a.clamp(0., 1.) * 255.).round() as u32) << 24 | 0xFFFFFF
}
/// Black at this opacity.
pub fn black(a: f32) -> u32 {
    ((a.clamp(0., 1.) * 255.).round() as u32) << 24
}
/// The same colour at another opacity.
pub fn alpha(c: u32, a: f32) -> u32 {
    ((a.clamp(0., 1.) * 255.).round() as u32) << 24 | (c & 0xFFFFFF)
}
/// Compose's `lerp(a, b, f)`, channel by channel.
pub fn lerp(a: u32, b: u32, f: f32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 255) as f32;
    let mix = |s: u32| ((ch(a, s) + (ch(b, s) - ch(a, s)) * f).round().clamp(0., 255.) as u32) << s;
    mix(24) | mix(16) | mix(8) | mix(0)
}

// ---------- type ----------
pub fn ts(spec: (f32, f32, f32), font: Font, color: u32, lines: u32) -> TextStyle {
    TextStyle { font, size: spec.0, color, line: spec.1, spacing: spec.2, lines, centre: false }
}
pub fn st(font: Font, size: f32, color: u32, lines: u32) -> TextStyle {
    TextStyle { font, size, color, line: 0., spacing: 0., lines, centre: false }
}
/// One line of text in a token's style.
pub fn line(s: impl Into<String>, spec: (f32, f32, f32), font: Font, color: u32) -> El {
    text(s, ts(spec, font, color, 1))
}
pub fn words(s: impl Into<String>, font: Font, size: f32, color: u32) -> El {
    text(s, st(font, size, color, 1))
}
pub fn centred(mut style: TextStyle) -> TextStyle {
    style.centre = true;
    style
}

// ---------- art colour ----------
// Every picture has a colour (the phone's ArtColor: hue weighted to its vivid, lit pixels, at a set brightness). The
// server works it out once per picture (/api/artcolor, as the website asks); asked a few at a time as cards appear.
/// The picture's colour if it's known; asked for if not (the page is drawn again once it comes). The core keeps and
/// asks for them (`zcore::art_answers`).
pub fn art_color(url: &str) -> Option<u32> {
    if url.is_empty() {
        return None;
    }
    let address = url.to_string();
    // (signed in: the server only answers its own)
    crate::images::COLOURS
        .answer(url.to_string(), async move {
            let answer = net::raw(&zcore::pictures::art_colour_path(&address)).await.ok()?;
            Some(i64::from(zcore::pictures::art_colour(&answer)))
        })
        .map(|c| c as u32)
}
/// The colour, or the quiet grey black-and-white art gets.
pub fn art_or_grey(url: &str) -> u32 {
    art_color(url).unwrap_or(t::C_NEUTRAL_GLOW)
}

// ---------- glass ----------
/// Liquid glass as a capsule: the preset's blur, its tint, and a 1 px rim lit at the top (w25 -> w06).
// The flat look (no glass): controls are solid shapes, a dark grey that reads over any picture; a coloured preset
// (the art's colour) is that colour, solid. Panels are solid dark cards with a hairline edge.
pub const FLAT_CONTROL: u32 = t::C_FLAT_CONTROL;
pub const FLAT_PANEL: u32 = t::C_PANEL;
pub const FLAT_LINE: u32 = t::C_FLAT_LINE;
/// A card: solid dark with a hairline edge.
pub fn panel(base: El, radius: f32) -> El {
    base.radius(radius).bg(FLAT_PANEL)
}
// ---------- the pieces ----------
/// Tappable: shrinks while pressed on the press spring (scale from tokens::PRESS_*).
pub fn tap(base: El, scale: f32, f: impl Fn(&mut App) + 'static) -> El {
    base.sink(scale).click(f)
}

// (buttons, chips, switches, rows, tiles, the page head and sheets are the design system's: ds.rs)

/// A row of chips that scrolls sideways and never wraps.
pub fn chip_row(key: &str, chips: Vec<El>, gutter: f32) -> El {
    row().key(key).fill_w().h(t::S_CHIP).scroll_x().gap(t::S_CHIP_GAP).pad4(gutter, 0., gutter, 0.).kids(chips)
}

/// A round flat button with an icon (a row's arrows).
pub fn round(key: &str, glyph: &'static str, size: f32, glyph_size: f32, f: impl Fn(&mut App) + 'static) -> El {
    let face = col().key(key).size(size, size).no_shrink().center().radius(t::R_PILL).bg(FLAT_CONTROL);
    tap(face.kid(icon(glyph, glyph_size, t::C_WHITE)), t::PRESS_ROUND, f)
}

/// An icon on nothing, with room around it to tap (the header's search).
pub fn bare_icon_button(key: &str, glyph: &'static str, size: f32, glyph_size: f32, color: u32, f: impl Fn(&mut App) + 'static) -> El {
    tap(col().key(key).size(size, size).no_shrink().center().kid(icon(glyph, glyph_size, color)), t::PRESS_ROUND, f)
}

/// "zun" ink + "flix" (#B3B3B1, or the art's colour in the header), bold, its letters drawn together by a share of its size.
pub fn wordmark(size: f32, flix: u32) -> El {
    let spec = (size, 0., -size * t::K_WORDMARK_SPACING_SHARE);
    // (the "zun" is measured with a little room past it, 1 + the spacing's: taken back so the word reads as one)
    let close = 1. + size * t::K_WORDMARK_SPACING_SHARE;
    row().items_center().kid(line("zun", spec, Font::Bold, t::C_INK).margin(0., 0., -close, 0.)).kid(line("flix", spec, Font::Bold, flix))
}

/// IMDb's yellow badge ("IMDb" in black, a share of the score's size) and the score in medium white.
pub fn imdb(rating: &str, size: f32) -> El {
    row().items_center().no_shrink()
        .kid(row().radius(t::R_IMDB).bg(t::C_IMDB).pad4(t::SPACE_4, t::K_CONTROL_BADGE_PAD_Y, t::SPACE_4, t::K_CONTROL_BADGE_PAD_Y)
            .kid(text("IMDb", TextStyle { spacing: t::K_IMDB_SPACING, ..st(Font::Bold, size * t::K_IMDB_TEXT_SHARE, t::C_BLACK, 1) })))
        .kid(words(rating, Font::Medium, size, t::C_WHITE).margin(t::SPACE_6, 0., 0., 0.))
}

/// The bootstrapper (installing, updating): a small window, zunflix's icon in the middle, under it what it's doing
/// and how far, in words (megabytes) and a thin bar. `share` None: no bar.
pub fn bootstrap_view(app: &App, title: &str, detail: &str, share: Option<f32>) -> El {
    bootstrap_view_over(app, title, detail, share, 0.)
}

/// The bootstrapper's panel with `foot` kept clear at its bottom (the uninstaller's box and buttons): the stack centred
/// in what's above it.
pub fn bootstrap_view_over(app: &App, title: &str, detail: &str, share: Option<f32>, foot: f32) -> El {
    let (width, height) = (app.kit.width, app.kit.height);
    // (an update's installer carries on in the window the app had: nothing fades in again there)
    let carrying_on = std::env::args().any(|a| a.starts_with("/UPDATE-PID="));
    // (one tight group in the middle, on true black: the app's own splash name ("z" in, the "f" dropping onto it, the
    // name opening out, a band of light across it), the thin bar right under it, then what it's doing and how far)
    let word = crate::pages::shell::splash_word(app, if carrying_on { f32::INFINITY } else { 0. }, t::K_BOOT_WORD);
    let said = zcore::setup::line(title, detail);
    let mut c = col().items_center().kid(word);
    // (the bar's room always kept, so going from asking to doing moves nothing: with no share it's faded out)
    let shown = share.map_or(0., |s| s.clamp(0., 1.));
    c = c.kid(el().key("boot-bar").w(t::K_BOOT_BAR).h(t::S_PROGRESS).radius(t::S_PROGRESS / 2.).bg(w(t::W15)).clip().margin(0., t::K_BOOT_BAR_GAP, 0., 0.)
        .opacity(if share.is_some() { 1. } else { 0. }).ease(320)
        .kid(el().at(0., 0., f32::NAN, 0.).pct_w(shown).bg(t::C_WHITE).radius(t::S_PROGRESS / 2.)));
    // (the line fades in with the bar when there's something to say, in its own kept room: nothing pops or moves)
    let quiet = said.trim().is_empty();
    c = c.kid(el().key("boot-line").margin(0., t::SPACE_14, 0., 0.).opacity(if quiet { 0. } else { 1. }).ease(320)
        .kid(line(&said, t::T_NOTE, Font::Medium, w(t::W60))));
    // (its own rounded card, rounder than Windows' 8: the window's corners are see-through (platform/windows/window.rs))
    // (behind it, the film-night pattern (reels, popcorn, tickets), faint)
    el().size(width, height).radius(BOOT_RADIUS).clip().bg(t::C_BG)
        .kid(el().cover().decorative().image("res:pattern"))
        .kid(col().at(0., 0., 0., foot).center().kid(c))
}

/// How round the bootstrapper's window is.
pub const BOOT_RADIUS: f32 = t::K_BOOT_RADIUS;

/// Loading: `#121212` in the shape of what's coming, a band of light sweeping across every 1.6 s.
pub fn skeleton(base: El, radius: f32) -> El {
    base.radius(radius).bg(t::C_SKELETON).clip().kid(
        shimmer_band(),
    )
}

/// The band of light a skeleton sweeps: 0.9 of the box wide, transparent -> shimmer -> transparent, left to right.
fn shimmer_band() -> El {
    el().at(0., 0., f32::NAN, 0.).pct_w(0.9).decorative().paint(Fill::Linear(0., 0., 1., 0., vec![(0., 0), (0.5, t::C_SHIMMER), (1., 0)])).shimmer()
}

/// A picture: the moving skeleton until it's in, then it fades in over 250 ms, cropped to fill.
pub fn picture(base: El, url: &str, radius: f32) -> El {
    let mut p = base.radius(radius).clip().bg(t::C_SKELETON);
    p = p.kid(shimmer_band());
    if !url.is_empty() {
        p = p.kid(el().cover().decorative().radius(radius).image(url.to_string()));
    }
    p
}

pub fn spinner(size: f32, color: u32) -> El {
    el().spinner(size, color).decorative()
}

/// Trouble: in the middle, the icon (offline or alert), the title, a muted line, then a white "Try again"
/// (a spinner in its place while it tries); the words are the core's (`zcore::nav::trouble_words`).
pub fn trouble(key: &str, message: &str, offline: bool, busy: bool, retry: impl Fn(&mut App) + 'static, downloads: Option<Cb>) -> El {
    let said = zcore::screen::Trouble { message: message.into(), offline, busy };
    let words_ = zcore::nav::trouble_words(&said, downloads.is_some());
    let (title, line_) = (words_.title.as_str(), words_.line.clone());
    let mut buttons = row().gap(t::SPACE_10).margin(0., t::K_CONTROL_TROUBLE_BUTTONS_GAP, 0., 0.).h(t::K_CONTROL_BUTTON_M);
    if busy {
        buttons = buttons.kid(col().size(t::K_CONTROL_BUTTON_M, t::K_CONTROL_BUTTON_M).center().kid(spinner(t::S_RING, t::C_INK)));
    } else {
        buttons = buttons.kid(tap(
            row().key(&format!("{key}-retry")).h(t::K_CONTROL_BUTTON_M).pad_xy(t::SPACE_20, 0.).center().radius(t::K_CONTROL_BUTTON_M / 2.).bg(t::C_WHITE)
                .kid(line(&words_.retry, t::T_BUTTON_MEDIUM, Font::Bold, t::C_BLACK)),
            t::PRESS_DEFAULT,
            retry,
        ));
        if let (Some(label), Some(go)) = (words_.downloads.clone(), downloads) {
            buttons = buttons.kid(tap(
                row().key(&format!("{key}-downloads")).h(t::K_CONTROL_BUTTON_M).pad_xy(t::SPACE_20, 0.).center().radius(t::K_CONTROL_BUTTON_M / 2.).bg(w(t::W12))
                    .kid(line(&label, t::T_BUTTON_MEDIUM, Font::Bold, t::C_INK)),
                t::PRESS_DEFAULT,
                move |a| go(a),
            ));
        }
    }
    col().key(key).fill().center().pad(t::K_CONTROL_MESSAGE_PAD)
        .kid(icon(if offline { "cloud_off" } else { "alert" }, t::K_CONTROL_TROUBLE_ICON, w(t::W85)))
        .kid(text(title, centred(ts(t::T_SECTION, Font::Bold, t::C_INK, 1))).margin(0., t::SPACE_16, 0., 0.))
        .kid(text(line_, centred(ts(t::T_TROUBLE, Font::Sans, t::C_MUTED, 3))).max_w(t::K_CONTROL_TROUBLE_LINE).margin(0., t::SPACE_6, 0., 0.))
        .kid(buttons)
}

/// Empty: the icon on a soft badge, the title, a muted line, centred.
pub fn empty(glyph: &'static str, title: &str, line_: &str) -> El {
    // (the icon solid, on a soft round badge: see-through strokes came out patchy where they crossed)
    col().fill_w().items_center().pad(t::K_CONTROL_MESSAGE_PAD)
        .kid(col().size(t::K_CONTROL_EMPTY_BADGE, t::K_CONTROL_EMPTY_BADGE).radius(t::K_CONTROL_EMPTY_BADGE / 2.).bg(t::C_EMPTY_BADGE).center()
            .kid(icon(glyph, t::K_CONTROL_EMPTY_ICON, t::C_EMPTY_ICON)))
        .kid(text(title, centred(ts(t::T_SECTION, Font::Bold, t::C_INK, 1))).margin(0., t::K_CONTROL_EMPTY_TITLE_GAP, 0., 0.))
        .kid(text(line_, centred(ts(t::T_TROUBLE, Font::Sans, t::C_MUTED, 3))).max_w(t::K_CONTROL_EMPTY_LINE).margin(0., t::SPACE_8, 0., 0.))
}

const HUES: [f32; 8] = [210., 150., 30., 330., 260., 190., 10., 90.];
fn hsl(h: f32, s: f32, l: f32) -> u32 {
    let c = (1. - (2. * l - 1.).abs()) * s;
    let x = c * (1. - ((h / 60.) % 2. - 1.).abs());
    let m = l - c / 2.;
    let (r, g, b) = match (h / 60.) as u32 {
        0 => (c, x, 0.),
        1 => (x, c, 0.),
        2 => (0., c, x),
        3 => (0., x, c),
        4 => (x, 0., c),
        _ => (c, 0., x),
    };
    let b8 = |v: f32| ((v + m) * 255.).round().clamp(0., 255.) as u32;
    0xFF000000 | b8(r) << 16 | b8(g) << 8 | b8(b)
}
/// A profile's picture, or a tile in its own hue with its first letter.
pub fn avatar(me: Option<&Me>, size: f32, radius: f32) -> El {
    let base = col().size(size, size).no_shrink().center().radius(radius).clip();
    let hue = HUES[me.map_or(0, |m| m.index) % HUES.len()];
    let letter = zcore::profiles::initial(me.map_or("", |m| m.name.as_str()));
    let tile = base.bg(hsl(hue, 0.14, 0.16)).kid(words(letter, Font::Bold, size * t::K_AVATAR_LETTER_SHARE, hsl(hue, 0.25, 0.92)));
    // (a picture: the moving skeleton until it's in, then it fades in — never the letter, a black square or the
    // picture it had flashing first; no picture: its letter tile)
    match me.and_then(|m| m.avatar.as_ref()) {
        Some(url) => skeleton(col().size(size, size).no_shrink().center(), radius).kid(el().cover().decorative().radius(radius).image(net::img(url)).animate_gif()),
        None => tile,
    }
}

// ---------- cards ----------
/// The picture a poster card shows.
/// A title's poster at its sharpest: metahub's large (780 across) for films and shows, ARY's big one for dramas;
/// shrunk to the card here, never stretched.
pub fn poster_url(item: &Item) -> String {
    zcore::pictures::poster(net::base(), item.poster_hd.as_deref(), item.poster.as_deref(), item.cover.as_deref(), item.fallback.as_deref())
}

/// PosterCard: 1 : 1.42, r22, #111 with a w08 rim; a shade in the poster's own colour from 52% down; the title 19 bold
/// (2 lines) and the year (or the facts, 220 wide and up) with the IMDb score, on the art.
/// Whether a title has its own clean artwork for a portrait card (TMDB's, or ARY's big portrait art) and a logo.
pub fn has_own_art(item: &Item) -> bool {
    item.own_art()
}

pub fn poster_card(key: &str, item: &Item, width: f32, f: impl Fn(&mut App) + 'static) -> El {
    poster_card_as(key, item, width, true, f)
}

/// A poster card; `facts`: the year and score fading in under the pointer (not in the Top 10: just the scale up).
pub fn poster_card_as(key: &str, item: &Item, width: f32, show_facts: bool, f: impl Fn(&mut App) + 'static) -> El {
    let url = poster_url(item);
    let shade = t::C_DARK;
    let h = width * t::S_POSTER_RATIO;
    let meta = item.poster_meta(width < t::K_CARD_POSTER_FACTS_MIN);
    // (art with no writing on it gets the title's logo over it; a poster that carries its own name, its name in bold)
    // (ARY's dramas too: their big portrait art is clean, and ARY gives each its logo)
    // (the phone's rule exactly: clean art gets its logo, else its name; Pakistan's dramas always carry their name; a
    // poster with its own title printed on it gets nothing over it, and no shade)
    let named = zcore::pictures::poster_named(item.clean_art(), item.source.as_deref());
    let logo = item.logo.as_deref().filter(|l| has_own_art(item) && !l.is_empty());
    let name: Option<El> = match logo {
        Some(l) => Some(logo_image(el().w(width - t::SPACE_14 * 2.).h(h * t::K_CARD_POSTER_LOGO_SHARE), l).backdrop(0., 250).centred()),
        None if named => Some(text(&item.title, TextStyle { centre: true, ..ts(t::T_CARD_TITLE, Font::Bold, t::C_INK, 2) }).fill_w()),
        None => None,
    };
    let shaded = name.is_some();
    // (its foot shade baked into the picture: one rounded layer, no seam at the corners)
    let _ = shade;
    let mut card = picture(col().key(key).size(width, h).no_shrink(), &url, t::R_POSTER_CARD).bg(t::C_CARD);
    if let Some(i) = card.kids.last_mut().and_then(|art| art.image.as_mut()).filter(|_| shaded) {
        i.shade = 2;
    }
    let card = card
        .kid(
            col().at(t::SPACE_14, f32::NAN, t::SPACE_14, t::SPACE_14).decorative()
                .maybe(name)
                // (the year and the score only under the pointer, fading in and out, with room under the logo)
                .maybe(show_facts.then(|| row().fill_w().margin(0., t::SPACE_10, 0., 0.).center().gap(t::SPACE_8).fade_on_hover(k(key), 0., 1.).ease(220)
                    .kid(text(meta, ts(t::T_META, Font::Medium, w(t::W75), 1)))
                    .maybe(item.rating.as_deref().map(|r| imdb(r, t::T_META.0))))),
        );
    tap(card.grow_hover(1.03), t::PRESS_DEFAULT, f)
}

// ---------- typing ----------
/// A typeable field: its key, the hint shown while it's empty, whether its words are hidden, and what's told of a
/// change and of Enter.
#[derive(Default)]
pub struct Field<'a> {
    pub key: &'a str,
    pub hint: &'a str,
    pub secret: bool,
    pub change: Option<Cb>,
    pub submit: Option<Cb>,
}

/// A field's text line: the words (or the hint), the selection and a blinking caret, kept in view as it's typed.
/// Returns the line and the spec the kit needs to make it typeable.
pub fn field_line(app: &App, field: Field, style: TextStyle, hint_color: u32, room: f32) -> (El, FieldSpec) {
    let Field { key, hint, secret, change, submit } = field;
    let focused = app.kit.focus == k(key);
    let edit = app.kit.edits.get(&k(key));
    let value = edit.map(|e| e.text.as_str()).unwrap_or_default();
    let shown = if secret { "•".repeat(value.encode_utf16().count()) } else { value.to_owned() };
    let caret = edit.filter(|_| focused).map_or(0., |e| app.kit.g.caret_x(&shown, &style, e.caret));
    let anchor = edit.filter(|e| focused && e.anchor != e.caret).map_or(caret, |e| app.kit.g.caret_x(&shown, &style, e.anchor));
    let offset = (caret - room + t::K_FIELD_CARET_ROOM).max(0.);
    let placeholder = shown.is_empty();
    let visible = if placeholder { hint.to_owned() } else { shown };
    let tint = if placeholder { hint_color } else { style.color };
    let height = (style.size * t::K_FIELD_LINE_RATIO).ceil();
    let mut l = el().fill_w().h(height).clip();
    if focused && (caret - anchor).abs() > 0.1 {
        l = l.kid(el().at(caret.min(anchor) - offset, 0., f32::NAN, 0.).w((caret - anchor).abs()).bg(w(t::W30)));
    }
    let width = app.kit.g.measure(&visible, &style, 1e5).0.max(1.);
    l = l.kid(text(visible, TextStyle { color: tint, ..style.clone() }).at(-offset, 0., f32::NAN, f32::NAN).w(width + t::K_FIELD_TEXT_SLACK));
    if focused {
        let mut bar = el().key("caret").size(t::K_FIELD_CARET, height - 2. * t::K_FIELD_CARET_INSET).at(caret - offset, t::K_FIELD_CARET_INSET, f32::NAN, f32::NAN).bg(t::C_WHITE);
        bar.blink = true;
        l = l.kid(bar);
    }
    (l, FieldSpec { style, secret, offset, change, submit })
}

/// GlassField: a capsule of w18 with a w20 rim; medium white words, the hint w60; an optional trailing button.
pub fn glass_field(app: &App, field: Field, width: f32, trailing: Option<El>) -> El {
    let key = field.key;
    let right = if trailing.is_some() { t::SPACE_6 } else { t::SPACE_20 };
    let room = width - t::SPACE_20 - right - if trailing.is_some() { t::K_FIELD_TRAILING_ROOM } else { 0. };
    let (l, spec) = field_line(app, field, ts(t::T_FIELD_TEXT, Font::Medium, t::C_WHITE, 1), w(t::W60), room);
    let mut f = row().key(key).w(width).h(t::S_FIELD).pad4(t::SPACE_20, 0., right, 0.).items_center().radius(t::S_FIELD / 2.).clip()
        .bg(w(t::W18)).border(t::S_HAIRLINE, if app.kit.focus == k(key) { w(t::W40) } else { w(t::W20) })
        .kid(l.grow());
    if let Some(button) = trailing {
        f = f.kid(button);
    }
    f.field = Some(spec);
    f
}

// ---------- a laptop's Home, as Netflix lays it out ----------
/// How wide the landscape cards are so a row shows `n` whole and a little of the next, running off the window's edge.
pub fn row_card_width(window: f32, gutter: f32) -> f32 {
    // (big enough to see on a laptop, as Netflix's: four and a peek on a laptop, five on a big screen)
    let n = if window >= t::K_HOME_ROW_FIVE_FROM { 5. } else if window >= t::K_HOME_ROW_FOUR_FROM { 4. } else if window >= t::K_HOME_ROW_THREE_FROM { 3. } else { 2. };
    ((window - gutter - t::S_CARD_GAP * n) / (n + t::K_HOME_ROW_PEEK_SHARE)).max(t::K_HOME_ROW_CARD_MIN)
}

/// A title's logo on a card, fitted, toned so it reads on the card's dark foot (a dark one brightened, a black one
/// lifted to white): a title with a logo always shows it, never its name instead.
fn logo_image(e: El, logo: &str) -> El {
    let url = net::img(logo);
    let (bright, lift) = crate::images::logo_tone(&url).unwrap_or((1., 0.));
    let mut e = e.image(url).contain();
    if let Some(i) = &mut e.image {
        i.bright = bright;
        i.lift = lift;
    }
    e
}

/// What this PC knows of the writing on pictures, for the core's card art.
struct ArtLooks;

impl zcore::art::Looks for ArtLooks {
    fn writing(&self, picture: &str, title: &str) -> Option<zcore::art::Writing> {
        crate::images::art_text(&net::img(picture), title)
    }

    fn latin(&self, logo: &str) -> Option<bool> {
        crate::images::latin_logo(&net::img(logo))
    }
}

/// A landscape card: the title's backdrop across it, its logo over the lower part (or its name when it has none and
/// its art carries none), a badge on its foot ("New Episode"), how far in along its bottom.
#[allow(clippy::too_many_arguments)]
pub fn landscape_card(key: &str, item: &Item, width: f32, badge: Option<&str>, top10: bool, progress: Option<f32>, info: Option<Cb>, f: impl Fn(&mut App) + 'static) -> El {
    let h = width * t::K_CARD_WIDE_RATIO;
    // Anime only: a backdrop with Japanese on it gives way to our own still of the show, and a logo in Japanese, or
    // over a backdrop that has the name already, isn't shown (the core's rules, zcore::art::card_art; the server reads
    // a picture once and remembers). Everything else is never held up by any of this.
    let art = zcore::art::card_art(
        &zcore::art::CardFacts {
            source: item.source.as_deref(),
            textless: item.textless,
            cover: item.cover.as_deref(),
            cover_big: item.cover_big.as_deref(),
            poster: item.poster.as_deref(),
            logo: item.logo.as_deref(),
            title: &item.title,
        },
        &ArtLooks,
    );
    let cover = art.cover.as_deref().unwrap_or_default();
    // (the larger cut, always shrunk to the card, never stretched: sharp; the server makes Home's ahead of time)
    let url = if cover.is_empty() { String::new() } else { net::card_img(cover, zcore::pictures::LANDSCAPE) };
    let r = t::K_CARD_LANDSCAPE_RADIUS;
    let mut card = picture(el().key(key).size(width, h).no_shrink(), &url, r).bg(t::C_CARD);
    // (a backdrop that can't be had falls back to the title's own picture: the still, or its poster)
    let fallback = zcore::pictures::landscape_fallback(item.cover.as_deref(), item.cover_big.as_deref(), item.poster.as_deref());
    if let (Some(f), Some(art)) = (fallback, card.kids.last_mut()) {
        if let Some(i) = &mut art.image {
            i.fallback = Some(net::img(f));
        }
    }
    // its name: the title's logo (an English one), else its name in bold, lower left over a shade
    let logo = art.logo.as_deref();
    let pad = (width * t::K_CARD_LANDSCAPE_PAD_SHARE).max(t::SPACE_12);
    // (room at the foot for how far in; a tag on the edge leaves the name where it is: it sits in the middle, the name
    // at the left, and they hardly ever meet)
    let foot = if progress.is_some() { t::SPACE_6 } else { 0. };
    // (what the name keeps clear of at the right: Play, and the info button beside it)
    let clear = t::K_CARD_ROUND_BUTTON + t::SPACE_6 + if info.is_some() { t::K_CARD_ROUND_BUTTON + t::SPACE_8 } else { 0. };
    let button = t::K_CARD_ROUND_BUTTON;
    let grown = button * t::K_CARD_LANDSCAPE_GROW_RATIO;
    // (resting: the art a touch dimmed; under the pointer it brightens, the details and Play fade in, a ring lights,
    // and it grows a little: all of it easing back out the same way when the pointer leaves)
    if let Some(i) = card.kids.last_mut().and_then(|art| art.image.as_mut()) {
        i.shade = 3;   // (its foot shade baked into the picture: one rounded layer, no seam at the corners)
    }
    let name: El = match logo {
        Some(l) => logo_image(el().w(width * t::K_CARD_LANDSCAPE_LOGO_W_SHARE).h(h * t::K_CARD_LANDSCAPE_LOGO_H_SHARE), l).backdrop(0., 250),
        // (Pakistan's dramas always carry their name, writing on their picture or not; others, unless their art has it)
        None if art.name => {
            let size = (width * t::K_CARD_LANDSCAPE_NAME_SHARE).clamp(t::K_CARD_LANDSCAPE_NAME_MIN, t::K_CARD_LANDSCAPE_NAME_MAX);
            text(&item.title, TextStyle { spacing: t::K_CARD_LANDSCAPE_NAME_SPACING, line: size * t::K_CARD_LANDSCAPE_NAME_LINE_RATIO, ..st(Font::Bold, size, t::C_WHITE, 2) })
                .w(width - pad * 2. - clear)
        }
        None => el(),
    };
    let detail = item.line();
    card = card.kid(col().at(pad, f32::NAN, pad + clear, pad + foot).items_start().decorative()
        .kid(name)
        .kid(text(detail, ts(t::T_FACTS, Font::Medium, w(t::W85), 1)).margin(0., t::SPACE_6, 0., 0.).fade_on_hover(k(key), 0., 1.).ease(240)));
    // (a margin moves a centred icon by half of it: Play's triangle nudged right to look centred)
    card = card.kid(col().at(f32::NAN, f32::NAN, pad - t::SPACE_2, pad - t::SPACE_2 + foot).size(button, button).radius(button / 2.).bg(t::C_WHITE).center().decorative()
        .kid(icon("play", t::K_CARD_PLAY_ICON, t::C_BLACK).margin(2. * t::K_CARD_PLAY_NUDGE, 0., 0., 0.)).fade_on_hover(k(key), 0., 1.).ease(240));
    if top10 {
        card = card.kid(top10_tag());
    }
    if let Some(label) = badge {
        card = card.kid(tag(label));
    }
    // (a card whose click plays (Continue watching): its title's page from the ⓘ beside Play, as big, fading in under
    // the pointer with it, or a right-click anywhere on it; drawn at Play's size with the card grown (1.04), as it only
    // shows then and doesn't grow with the card itself)
    if let Some(info) = &info {
        let go = info.clone();
        card = card.kid(tap(col().key(&format!("{key}-info")).at(f32::NAN, f32::NAN, pad - t::SPACE_2 + grown + t::SPACE_8, pad - t::SPACE_2 + foot).size(grown, grown).radius(grown / 2.).bg(w(t::W24)).center()
            .kid(icon("info", t::K_CARD_LANDSCAPE_INFO_ICON, t::C_WHITE)).fade_on_hover(k(key), 0., 1.).ease(240), t::PRESS_ROUND, move |a| go(a)));
    }
    let mut card = tap(card.grow_hover(t::K_CARD_LANDSCAPE_GROW_RATIO), t::PRESS_DEFAULT, f);
    if let Some(info) = info {
        card = card.right_click(move |a| info(a));
    }
    // how far in: a thin bar under the card, as Netflix's
    match progress {
        Some(p) => col().key(&format!("{key}-with-bar")).w(width).no_shrink().items_center().kid(card)
            .kid(el().w(width * t::K_CARD_PROGRESS_SHARE).h(t::S_PROGRESS).margin(0., t::K_CARD_PROGRESS_GAP, 0., 0.).radius(t::S_PROGRESS / 2.).clip().bg(w(t::W25)).kid(el().pct_w(p.clamp(0., 1.)).fill_h().bg(crate::theme::accent()))),
        None => card,
    }
}

/// In the Top 10: a small white banner (the tags' white) hanging from the card's top edge a little in from the right, a tiny
/// "TOP" over a bigger "10" (its top corners run under the edge, so only the lower ones show rounded).
fn top10_tag() -> El {
    let ink = crate::theme::ink();
    let inner = t::K_CARD_TOP_MARK_WIDTH - t::K_CARD_TOP_MARK_PAD_END;
    col().at(f32::NAN, -t::K_CARD_TAG_RADIUS, t::SPACE_10, f32::NAN).w(t::K_CARD_TOP_MARK_WIDTH).pad4(0., t::SPACE_6, t::K_CARD_TOP_MARK_PAD_END, t::SPACE_4)
        .radius(t::K_CARD_TAG_RADIUS).bg(t::C_WHITE).items_center().decorative()
        // (each line the box's whole width, its words centred in it: a measured box left the "10" off to one side)
        .kid(text(zcore::cards::TOP_LABEL, centred(ts(t::T_TOP_MARK_LABEL, Font::Bold, ink, 1))).w(inner))
        .kid(text(zcore::cards::TOP_NUMBER, centred(ts(t::T_TOP_MARK_NUMBER, Font::Bold, ink, 1))).w(inner))
}

/// A card's tag face: white with bold black words.
fn tag_face(label: &str) -> El {
    row().h(t::K_CARD_TAG_HEIGHT).pad4(t::K_CARD_BADGE_PAD, 0., t::K_CARD_BADGE_PAD, 0.).center().bg(t::C_WHITE)
        .kid(line(label, t::T_CARD_TAG_LABEL, Font::Bold, crate::theme::ink()))
}

/// What's new about it, sized and placed as Netflix tags it: white with black words, flush with the card's foot in
/// the middle (its lower corners run under the edge, so only the top ones show rounded).
fn tag(label: &str) -> El {
    let stack = col().radius(t::K_CARD_TAG_RADIUS).clip().kid(tag_face(label));
    row().at(0., f32::NAN, 0., -t::K_CARD_TAG_RADIUS).center().decorative().kid(stack.kid(el().h(t::K_CARD_TAG_RADIUS)))
}

/// A Top 10 place, as Netflix's: the number as a hollow figure, as tall as the poster, the poster over its right
/// over its edge; the poster lifts and rings as the others do under the pointer.
pub fn rank_cell(key: &str, item: &Item, rank: usize, width: f32, f: impl Fn(&mut App) + 'static) -> El {
    let poster_h = width * t::K_HOME_ROW_RANKED_RATIO;
    let poster_w = poster_h / t::S_POSTER_RATIO;
    // the number's box, as tall as the poster and as wide as its figure's (85 or 88 of 148), right against the poster
    let room = poster_h * crate::rank_digits::FIGURES[(rank.clamp(1, 10)) - 1].width / crate::rank_digits::HEIGHT;
    // (the place just its number and its poster: the 1 right at the row's start, room before each later number)
    // (the 1's own drawing starts 29.75 into its box: that blank is cut too, so its stroke meets the row's edge)
    let blank = if rank == 1 { poster_h * 29.75 / crate::rank_digits::HEIGHT } else { 0. };
    let width = room + poster_w + if rank == 1 { 0. } else { t::SPACE_28 } - blank;
    // (the poster is every row's poster card: the same picture, logo and name rules, nothing of its own)
    el().key(key).size(width, poster_h).no_shrink()
        .kid(el().key(&format!("{key}-n")).at(width - poster_w - room, 0., f32::NAN, f32::NAN).size(room, poster_h).decorative().rank_figure(rank))
        .kid(el().at(width - poster_w, 0., f32::NAN, f32::NAN).size(poster_w, poster_h).kid(poster_card_as(&format!("{key}-art"), item, poster_w, false, f)))
}
