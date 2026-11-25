//! zunflix's design system for Windows: the tokens and the pieces every screen is built from, so the same thing always
//! looks and behaves the same. Build with these; don't make one-off buttons, rows, colours or headers.
//!
//! Tokens: page and three surface levels, three text levels, accent (and what's written on it), danger (and its soft
//! fill), radii, spacing, type, motion. Pieces: Button (Primary / Secondary / Ghost / Danger, sizes S 32, M 40, L 46,
//! an optional Lucide icon), the destructive action with its confirm step, IconButton, Chip, Segmented, Switch,
//! Tile, List with its Rows and Dividers, LeadIcon, SettingLine, Choice, Badge, the page Head (with or without
//! Back), the Sheet card and its backdrop.
use crate::gfx::{Font, TextStyle};
use crate::kit::{col, el, icon, k, row, text, Cb, El, Fill};
use crate::tokens as t;
use crate::ui::{alpha, line, tap, ts, w};
use crate::App;
use std::rc::Rc;

// ================= tokens =================
/// Surfaces, one step up each: 1 tiles and cards, 2 controls and lifted rows, 3 pressed or chosen.
pub fn surface(level: u8) -> u32 {
    w(match level { 0 => 0.035, 1 => 0.05, 2 => 0.08, _ => 0.12 })
}
/// Text: primary (white), secondary, tertiary.
pub const TEXT: u32 = t::C_WHITE;
pub fn text2() -> u32 { w(t::W70) }
pub fn text3() -> u32 { w(t::W50) }
/// The accent (what's on, badges, progress) and what's written on it.
pub fn accent() -> u32 { crate::theme::accent() }
pub fn on_accent() -> u32 { crate::theme::ink() }
/// Danger: deleting, removing, signing out, errors. Its soft fill is the resting state of a destructive button.
pub const DANGER: u32 = crate::theme::DANGER;
pub fn danger_soft() -> u32 { alpha(DANGER, 0.12) }
/// Radii.
pub const R_LG: f32 = t::K_CONTROL_TILE_RADIUS;
/// Spacing.
pub const SP_S: f32 = t::SPACE_8;
pub const SP_M: f32 = t::SPACE_12;
pub const SP_L: f32 = t::SPACE_16;
/// Type: a caption.
pub fn caption_style(color: u32, lines: u32) -> TextStyle { ts(t::T_NOTE, Font::Sans, color, lines) }
/// Motion: a hover light easing in and out over this long.
pub const HOVER_MS: i64 = 220;

/// Whether the pointer is over the piece with this key.
pub fn hot(app: &App, key: &str) -> bool {
    app.kit.hover_within(k(key))
}

/// A light over a piece while the pointer is on it, easing in and out (under its words and icon).
fn hover_light(radius: f32, color: u32, on: bool) -> El {
    el().cover().decorative().radius(radius).bg(color).opacity(if on { 1. } else { 0. }).ease(HOVER_MS)
}

// ================= buttons =================
#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// The main action: white, black words (Play, Save, Sign in).
    Primary,
    /// The other action beside it: a lifted surface, white words (More info, Cancel, Manage profiles).
    Secondary,
    /// Quiet: nothing until the pointer's on it (Remove picture, Back to the code).
    Ghost,
    /// Destructive: soft red, red words (Delete, Remove, Sign out); see `danger` for the confirm step.
    Danger,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Size { S, M, L }

impl Size {
    pub fn h(self) -> f32 { match self { Size::S => t::K_CONTROL_BUTTON_S, Size::M => t::K_CONTROL_BUTTON_M, Size::L => t::K_CONTROL_BUTTON_L } }
    fn text(self) -> (f32, f32, f32) { match self { Size::S => t::T_BUTTON_SMALL, Size::M => t::T_BUTTON_MEDIUM, Size::L => t::T_PILL } }
    fn glyph(self) -> f32 { match self { Size::S => t::K_CONTROL_BUTTON_ICON_S, Size::M => t::K_CONTROL_BUTTON_ICON_M, Size::L => t::K_CONTROL_BUTTON_ICON_L } }
    fn pad(self) -> f32 { match self { Size::S => t::SPACE_12, Size::M => t::SPACE_16, Size::L => t::K_CONTROL_BUTTON_PAD_L } }
}

fn colours(kind: Kind) -> (u32, u32, u32) {
    match kind {
        Kind::Primary => (t::C_WHITE, t::C_BLACK, t::C_PRIMARY_PRESS),
        Kind::Secondary => (surface(2), TEXT, w(t::W08)),
        Kind::Ghost => (0, TEXT, w(t::W08)),
        Kind::Danger => (danger_soft(), DANGER, alpha(DANGER, 0.1)),
    }
}

/// A button: a capsule in its kind's colours, an optional Lucide icon before the words, a light easing in and out
/// under the pointer, sinking a touch while pressed.
pub fn button(app: &App, key: &str, kind: Kind, size: Size, glyph: Option<&'static str>, label: impl Into<String>, f: impl Fn(&mut App) + 'static) -> El {
    button_at(app, key, kind, size, glyph, false, 0., label, f)
}

/// A button whose icon comes after its words, for one that goes on (Next chapter ›), or a fold's (its chevron turned
/// `turn` degrees).
#[allow(clippy::too_many_arguments)]
pub fn button_onward(app: &App, key: &str, kind: Kind, size: Size, glyph: &'static str, turn: f32, label: impl Into<String>, f: impl Fn(&mut App) + 'static) -> El {
    button_at(app, key, kind, size, Some(glyph), true, turn, label, f)
}

#[allow(clippy::too_many_arguments)]
fn button_at(app: &App, key: &str, kind: Kind, size: Size, glyph: Option<&'static str>, after: bool, turn: f32, label: impl Into<String>, f: impl Fn(&mut App) + 'static) -> El {
    let (bg, ink, light) = colours(kind);
    let h = size.h();
    let glyph = |at: bool| glyph.filter(|_| at == after).map(|g| icon(g, size.glyph(), ink).key(&format!("{key}-glyph")).turn(turn));
    tap(
        row().key(key).h(h).pad_xy(size.pad(), 0.).radius(h / 2.).center().gap(t::SPACE_8).no_shrink().bg(bg)
            .kid(hover_light(h / 2., light, hot(app, key)))
            .maybe(glyph(false))
            .kid(line(label, size.text(), Font::Bold, ink))
            .maybe(glyph(true)),
        t::PRESS_CHIP,
        f,
    )
}

/// The destructive action, one way everywhere: a soft red button (`label`); clicked once it turns solid red and asks
/// (`confirm`, e.g. "Click again to delete"); the caller does it on the second click (`asking` is that state).
#[allow(clippy::too_many_arguments)]
pub fn danger(app: &App, key: &str, size: Size, glyph: Option<&'static str>, label: &str, confirm: &str, asking: bool, f: impl Fn(&mut App) + 'static) -> El {
    if !asking {
        return button(app, key, Kind::Danger, size, glyph, label, f);
    }
    let h = size.h();
    tap(
        row().key(key).h(h).pad_xy(size.pad(), 0.).radius(h / 2.).center().gap(t::SPACE_8).no_shrink().bg(DANGER)
            .maybe(glyph.map(|g| icon(g, size.glyph(), t::C_WHITE)))
            .kid(line(confirm, size.text(), Font::Bold, t::C_WHITE)),
        t::PRESS_CHIP,
        f,
    )
}

/// The destructive action where there's only room for its icon (a download's delete): a soft red disc; clicked once
/// it turns into the solid red button asking (`confirm`); the caller does it on the second click.
pub fn danger_icon(app: &App, key: &str, size: Size, glyph: &'static str, confirm: &str, asking: bool, f: impl Fn(&mut App) + 'static) -> El {
    // (how far it's grown: 0 the disc, 1 the pill; eased over 240 ms from when it last changed, laid out every frame
    // meanwhile, so what's beside it gives way smoothly)
    let p = {
        let mut morphs = app.pages.morphs.borrow_mut();
        let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(5);
        let at = morphs.entry(key.to_string()).or_insert((asking, long_ago));
        if at.0 != asking {
            *at = (asking, std::time::Instant::now());
        }
        let k = crate::kit::bezier(t::EASE_EASE, (at.1.elapsed().as_secs_f32() * 1000. / 240.).min(1.));
        if asking { k } else { 1. - k }
    };
    if p <= 0. {
        return icon_button(app, key, Kind::Danger, size, glyph, f);
    }
    let h = size.h();
    // (the pill's width from its words: Geist bold runs about .56 of its size a letter; a little over, never cut short)
    let full = confirm.chars().count() as f32 * size.text().0 * 0.58 + size.pad() * 2.;
    let (soft, _, _) = colours(Kind::Danger);
    // (the disc's own room stays as it is in the row: the pill grows leftwards over what's beside it (a name), so
    // nothing there wraps or moves; opaque underneath, so what it covers doesn't show through)
    let pill = tap(
        el().key(key).at(f32::NAN, 0., 0., f32::NAN).w(h + (full - h) * p).h(h).radius(h / 2.).clip().bg(t::C_BG)
            .kid(el().cover().decorative().bg(soft))
            .kid(el().cover().decorative().bg(DANGER).opacity(p))
            .kid(col().cover().center().decorative().opacity(1. - p).kid(icon(glyph, size.glyph() + t::K_CONTROL_ICON_BUTTON_EXTRA, DANGER)))
            .kid(row().cover().center().decorative().opacity(p).kid(line(confirm, size.text(), Font::Bold, t::C_WHITE))),
        t::PRESS_CHIP,
        f,
    );
    el().key(&format!("{key}-room")).size(h, h).no_shrink().kid(pill)
}

/// A round button with just an icon, in a button's kind (Ghost for the quiet ones on rows and bars).
pub fn icon_button(app: &App, key: &str, kind: Kind, size: Size, glyph: &'static str, f: impl Fn(&mut App) + 'static) -> El {
    let (bg, ink, light) = colours(kind);
    let h = size.h();
    tap(
        col().key(key).size(h, h).radius(h / 2.).center().no_shrink().bg(bg)
            .kid(hover_light(h / 2., light, hot(app, key)))
            .kid(icon(glyph, size.glyph() + t::K_CONTROL_ICON_BUTTON_EXTRA, ink)),
        t::PRESS_ROUND,
        f,
    )
}

/// A page head's icon (search, settings): the medium ghost button, its glyph a touch bigger so it reads in the bar.
pub fn head_button(app: &App, key: &str, glyph: &'static str, f: impl Fn(&mut App) + 'static) -> El {
    let (bg, ink, light) = colours(Kind::Ghost);
    let h = Size::M.h();
    tap(
        col().key(key).size(h, h).radius(h / 2.).center().no_shrink().bg(bg)
            .kid(hover_light(h / 2., light, hot(app, key)))
            .kid(icon(glyph, t::K_CONTROL_HEAD_ICON, ink)),
        t::PRESS_ROUND,
        f,
    )
}

// ================= choosing =================
/// A chip (a section's filters): white with black words when on; a soft surface with a light under the pointer.
pub fn chip(app: &App, key: &str, label: impl Into<String>, on: bool, f: impl Fn(&mut App) + 'static) -> El {
    badged_chip(app, key, label, None, on, f)
}

/// A chip with a small rounded badge after its words (the player's Quality: an Easynews copy's "U"). The badge is
/// white with bold black words, as the cards' tags; on a chosen chip, the other way round.
pub fn badged_chip(app: &App, key: &str, label: impl Into<String>, badge: Option<&str>, on: bool, f: impl Fn(&mut App) + 'static) -> El {
    let h = t::S_CHIP;
    let (ground, ink) = if on { (on_accent(), accent()) } else { (t::C_WHITE, t::C_BLACK) };
    let badge = badge.map(|b| row().h(t::K_CONTROL_CHIP_BADGE).pad_xy(t::SPACE_6, 0.).radius(t::K_CONTROL_CHIP_BADGE / 2.).center().no_shrink()
        .margin(t::K_CONTROL_CHIP_BADGE_GAP, 0., 0., 0.).bg(ground).kid(line(b, t::T_CHIP_BADGE, Font::Bold, ink)));
    tap(
        row().key(key).h(h).no_shrink().pad_xy(t::SPACE_16, 0.).center().radius(h / 2.).bg(if on { accent() } else { w(t::W09) })
            .kid(hover_light(h / 2., w(t::W07), !on && hot(app, key)))
            .kid(line(label, t::T_CHIP, Font::Bold, if on { on_accent() } else { w(t::W80) }))
            .maybe(badge),
        t::PRESS_CHIP,
        f,
    )
}

/// Choices side by side in a soft track that hugs them, on one line; the chosen one white with black words.
pub fn segmented<V: AsRef<str> + Clone + 'static>(app: &App, key: &str, options: &[(V, &str)], current: &str, f: impl Fn(&mut App, V) + 'static) -> El {
    let f = Rc::new(f);
    let mut c = row().key(key).h(t::K_SETTINGS_SEGMENT + 2. * t::SPACE_4).pad(t::SPACE_4).radius(t::K_SETTINGS_SEGMENT / 2. + t::SPACE_4).gap(t::SPACE_4).bg(surface(0)).items_center().no_shrink();
    for (value, label) in options {
        let on = value.as_ref() == current;
        let k_ = format!("{key}-{}", value.as_ref());
        let (f, v) = (f.clone(), value.clone());
        c = c.kid(tap(
            row().key(&k_).h(t::K_SETTINGS_SEGMENT).pad_xy(t::K_SETTINGS_SEGMENT_PAD, 0.).radius(t::K_SETTINGS_SEGMENT / 2.).center().no_shrink().bg(if on { t::C_WHITE } else { 0 })
                .kid(hover_light(t::K_SETTINGS_SEGMENT / 2., w(t::W08), !on && hot(app, &k_)))
                .kid(line(*label, t::T_SETTINGS_CHOICE, Font::Bold, if on { t::C_BLACK } else { w(t::W75) })),
            t::PRESS_CHIP,
            move |a| f(a, v.clone()),
        ));
    }
    c
}

/// A switch: a track, its knob sliding across; white when on.
pub fn switch(app: &App, key: &str, on: bool, f: impl Fn(&mut App) + 'static) -> El {
    let (track_w, track_h, knob) = (t::K_SETTINGS_SWITCH_WIDTH, t::K_SETTINGS_SWITCH_HEIGHT, t::K_SETTINGS_SWITCH_KNOB);
    let inset = (track_h - knob) / 2.;
    tap(
        el().key(key).size(track_w, track_h).radius(track_h / 2.).no_shrink().bg(if on { t::C_WHITE } else if hot(app, key) { w(t::W26) } else { w(t::W18) })
            .kid(el().key(&format!("{key}-knob")).at(if on { track_w - knob - inset } else { inset }, inset, f32::NAN, f32::NAN).size(knob, knob).radius(knob / 2.)
                .bg(if on { t::C_BLACK } else { t::C_WHITE }).slide(180).decorative()),
        t::PRESS_CHIP,
        f,
    )
}

// ================= structure =================
/// A tile (Settings): rounded LG on surface 1, its icon in a soft square, its title and a quiet helper, then its body.
pub fn tile(glyph: &'static str, name: &str, helper: &str, width: f32, body: El) -> El {
    col().w(width).no_shrink().pad(t::K_SETTINGS_TILE_PAD).radius(R_LG).bg(surface(1))
        .kid(row().fill_w().items_center().gap(SP_M).margin(0., 0., 0., t::K_SETTINGS_TILE_HEAD_GAP)
            .kid(lead_icon_sized(glyph, t::K_SETTINGS_TILE_ICON))
            .kid(col().grow()
                .kid(line(name, t::T_GROUP_HEADING, Font::Bold, TEXT))
                .kid(text(helper, caption_style(text3(), 1)).fill_w().margin(0., t::SPACE_2, 0., 0.))))
        .kid(body)
}

/// A list: rows grouped in one rounded card with lines between them.
pub fn list(inner: El) -> El {
    inner.radius(t::K_SETTINGS_LIST_RADIUS).clip().bg(surface(0))
}

/// The line between two rows of a list, starting under the rows' names.
pub fn divider() -> El {
    el().fill_w().h(t::S_HAIRLINE).bg(crate::ui::FLAT_LINE).margin(SP_L + t::K_SETTINGS_LEAD_ICON + SP_L, 0., 0., 0.)
}

/// A row of a list: what it's of (a picture, or a LeadIcon), its name and a quiet line, a chevron when it goes
/// somewhere (or `end`: a button, a check); a light under the pointer when it does something.
pub fn list_row(app: &App, key: &str, lead: El, label: &str, value: Option<String>, go: Option<Cb>, end: Option<El>) -> El {
    let on = go.is_some() && hot(app, key);
    let mut r = row().key(key).fill_w().min_h(t::K_SETTINGS_LIST_ROW).pad4(SP_L, SP_M, SP_L, SP_M).items_center().gap(SP_L)
        .kid(hover_light(0., w(t::W04), on))
        .kid(lead)
        .kid(col().grow()
            .kid(text(label, ts(t::T_ENTRY_NAME, Font::Bold, TEXT, 1)).fill_w())
            .maybe(value.map(|v| text(v, caption_style(w(t::W55), 1)).fill_w().margin(0., t::K_CONTROL_LINE_GAP, 0., 0.))));
    let goes = go.is_some();
    if let Some(e) = end {
        r = r.kid(e);
    } else if goes {
        r = r.kid(icon("chevron_right", t::K_CONTROL_CHEVRON, w(if on { t::W80 } else { t::W40 })));
    }
    match go {
        Some(f) => tap(r, t::PRESS_ROW, move |a| f(a)),
        None => r,
    }
}

/// An icon in a soft square, leading a row.
pub fn lead_icon(glyph: &'static str) -> El {
    lead_icon_sized(glyph, t::K_SETTINGS_LEAD_ICON)
}
fn lead_icon_sized(glyph: &'static str, size: f32) -> El {
    col().size(size, size).radius(size * t::K_SETTINGS_LEAD_CORNER_SHARE).bg(surface(2)).center().no_shrink()
        .kid(icon(glyph, size * t::K_SETTINGS_LEAD_GLYPH_SHARE, w(t::W90)))
}

/// A setting's name (and a quiet line) with its control at the right.
pub fn setting_line(name: &str, helper: Option<&str>, control: El) -> El {
    row().fill_w().items_center().gap(t::SPACE_14)
        .kid(col().grow()
            .kid(line(name, t::T_SETTING_NAME, Font::Bold, TEXT))
            .maybe(helper.map(|h| text(h, caption_style(text3(), 2)).fill_w().margin(0., t::K_CONTROL_LINE_GAP, 0., 0.))))
        .kid(control)
}

/// A setting's name above its choices (a Segmented).
pub fn choice(name: &str, control: El) -> El {
    col().fill_w().gap(t::SPACE_10).items_start().kid(line(name, t::T_SETTING_NAME, Font::Bold, TEXT)).kid(control)
}

/// A small pill of information (an update waiting, a count): surface 2, an optional icon, bold caption words.
pub fn badge(glyph: Option<&'static str>, label: impl Into<String>) -> El {
    row().h(t::K_SETTINGS_BADGE).pad_xy(t::SPACE_12, 0.).radius(t::K_SETTINGS_BADGE / 2.).items_center().gap(t::K_SETTINGS_BADGE_GAP).bg(surface(2)).no_shrink()
        .maybe(glyph.map(|g| icon(g, t::K_SETTINGS_SMALL_GLYPH, TEXT)))
        .kid(line(label, t::T_NOTE, Font::Bold, TEXT))
}

/// An error or a note under a control: danger red, centred when asked.
pub fn error_text(message: &str, centre: bool) -> El {
    let style = caption_style(DANGER, 3);
    text(message, if centre { crate::ui::centred(style) } else { style })
}

// ================= head and sheets =================
/// Where a page's content starts: below its head, with a little room.
pub const HEAD: f32 = t::K_NAV_PAGE_HEAD + t::SPACE_12;


/// How big the back button is: a search field's height, so the two sit level beside each other.
pub const BACK: f32 = t::S_FIELD_SEARCH;

/// The one back button: a 48 soft disc (a search field's height) with Lucide's arrow, a light under the pointer.
pub fn back_button(app: &App, key: &str, f: impl Fn(&mut App) + 'static) -> El {
    let on = hot(app, key);
    tap(
        // (a see-through white, so it stands out on any ground: the gradient, a picture, a scrolled bar)
        col().key(key).size(BACK, BACK).no_shrink().radius(BACK / 2.).center().bg(crate::ui::w(if on { 0.2 } else { 0.11 })).ease(160)
            .kid(icon("arrow_left", t::K_CONTROL_HEAD_ICON, TEXT)),
        t::PRESS_ROUND,
        f,
    )
}

/// The head every screen has: (Back, for pages opened over others), its name, anything it adds at the right; a solid
/// ground behind it once the page has scrolled under it. `left`: the page's gutter. Content starts at HEAD.
#[allow(clippy::too_many_arguments)]
pub fn head(app: &App, key: &str, left: f32, back: Option<Cb>, name: Option<&str>, scrolled: bool, trailing: Option<El>) -> El {
    let mut bar = el().key(key).at(0., 0., 0., f32::NAN).h(t::K_NAV_PAGE_HEAD);
    // (its ground fades in as the page scrolls under it, and back out as it returns to the top; no line under it)
    // (its ground is the window's own: the very gradient and glow behind the rail, cut out where the bar is and laid
    // exactly where they are, so beside the rail there's nothing to see where it ends; the page just goes under it)
    let rail = app.pages.rail_width();
    // (up under the window's strip too: from the window's top, so its cut of the window's ground lines up exactly)
    let mut ground = el().key(&format!("{key}-ground")).at(0., -crate::WINDOW_STRIP, 0., f32::NAN).h(t::K_NAV_PAGE_HEAD + crate::WINDOW_STRIP).clip().decorative()
        .opacity(if scrolled { 1. } else { 0. }).ease(260)
        .kid(el().key(&format!("{key}-ground-g")).at(-rail, 0., f32::NAN, f32::NAN).size(app.kit.width, app.kit.height).decorative()
            .image("res:backdrop").backdrop(0.3, 0));
    if let Some((name, now, before, strength)) = app.pages.current_glow(app) {
        ground = ground.kid(el().key(&format!("{key}-glow")).at(-rail, 0., f32::NAN, f32::NAN).size(app.kit.width, app.kit.height).decorative()
            .opacity(strength).ease(crate::pages::shell::SCROLL_EASE).kid(crate::pages::glow_layers(&format!("{key}-glow-{name}"), now, before, 0.)));
    }
    bar = bar.kid(ground);
    if scrolled {
        // (a click on the solid bar stops at it)
        let mut catcher = el().cover().click(|_| {});
        catcher.hand = false;
        bar = bar.kid(catcher);
    }
    let page_w = app.kit.width - rail;
    let mut line_ = row().at(left, t::SPACE_6, f32::NAN, f32::NAN).w((page_w - left * 2.).max(0.)).h(Size::M.h()).items_center();
    if let Some(b) = back {
        line_ = line_.kid(back_button(app, &format!("{key}-back"), move |a| b(a)).margin(0., 0., SP_L, 0.));
    }
    if let Some(n) = name {
        line_ = line_.kid(text(n, ts(t::T_PAGE_HEAD, Font::Bold, TEXT, 1)));
    }
    bar.kid(line_.kid(el().grow()).maybe(trailing.map(|t| t.no_shrink())))
}

/// A sheet's card (Edit profile, Sign in a TV): lit from the top, a faint highlight along it.
pub fn sheet_card(width: f32, body: El) -> El {
    col().w(width).pad4(t::SPACE_28, t::SPACE_28, t::K_CONTROL_SHEET_PAD_END, t::K_CONTROL_SHEET_PAD_BOTTOM).radius(t::K_PAGE_DIALOG_RADIUS)
        .paint(Fill::Linear(0., 0., 0., 1., vec![(0., t::C_SHEET_TOP), (1., t::C_SHEET_FOOT)])).top_line(w(t::W10)).kid(body)
}

/// What's behind a sheet or a menu: the page blurred and dimmed.
pub fn sheet_backdrop() -> El {
    el().cover().blur(14.).kid(el().cover().decorative().bg(crate::ui::black(0.35)))
}

/// A menu's card (the profile switcher): lit from the top, a faint highlight.
pub fn menu_card(width: f32) -> El {
    col().w(width).pad(SP_S).radius(t::K_SIDEBAR_MENU_CORNER).bg(t::C_RAISED)
}
