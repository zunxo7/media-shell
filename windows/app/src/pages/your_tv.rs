//! Your TV, as the phone has it: playing on the account's TV from this PC. The TV bar at the foot of every page while
//! Play on TV is on, and the keys that work the TV from the Your TV page. Everything it says and does is the core's
//! (`zcore::your_tv`); this draws its view.
use super::shell::{gutter, page_head, HEAD};
use super::*;
use zcore::remote::Key;
use zcore::your_tv::{Event, Playing};

/// From this wide the page lays what's on beside its controls, as Settings lays its tiles in two columns.
const TWO_COLUMNS: f32 = t::K_YOUR_TV_TWO_COLUMNS_MIN;

impl Pages {
    /// Opens Your TV, or Sign in a TV while the account has none (the rail's entry: the core's navigation says which).
    pub(super) fn open_your_tv(&mut self) {
        self.clear_for();
        self.your_tv.send(Event::Opened);
        self.push(Route::YourTv);
    }

    /// Your TV closes once its TV is signed out; its clock carries on while the TV plays.
    pub(super) fn your_tv_tick(&mut self, kit: &mut crate::kit::Kit) {
        let (closed, moving) = {
            let v = self.your_tv.view();
            (v.closed, v.playing.as_ref().is_some_and(|p| p.moving))
        };
        let here = self.route() == Some(&Route::YourTv);
        if closed != self.tv_closed {
            self.tv_closed = closed;
            if here {
                self.back();
            }
        }
        if here && moving {
            kit.dirty = true;
        }
    }

    /// Your TV: the TV's card (Play on TV, and Change TV and Sign out TV under them); what's on (its picture, title,
    /// where), how far along (drag to move), back and forward 10 round play/pause, Next, the TV's volume between its
    /// subtitles and settings (each only when there's something to pick), Stop. On a wide window what's on sits beside
    /// its controls.
    pub(super) fn your_tv_view(&self, app: &App) -> El {
        let width = view_w(app);
        let gutter = gutter(width);
        let pos = app.kit.scroll_of(k("page-your-tv")).pos;
        let content = width - gutter * 2.;
        let two = content >= TWO_COLUMNS;
        let grid_w = if two { content.min(t::L_LARGE_CONTENT_MAX) } else { content.min(t::L_COMPACT_TEXT_MAX) };
        let v = self.your_tv.view();
        let mut body = col().w(grid_w).no_shrink().kid(self.tv_tile(app));
        match &v.playing {
            Some(now) if two => {
                let left_w = ((grid_w - t::SPACE_32) * t::K_YOUR_TV_NOW_SHARE).floor();
                body = body.kid(row().fill_w().items_start().gap(t::SPACE_32).margin(0., t::SPACE_24, 0., 0.)
                    .kid(col().w(left_w).no_shrink().kid(now_art(left_w, now)).kid(now_words(now)))
                    .kid(self.now_controls(app, now, grid_w - left_w - t::SPACE_32)));
            }
            Some(now) => {
                body = body.kid(col().fill_w().margin(0., t::SPACE_16, 0., 0.)
                    .kid(now_art(grid_w, now)).kid(now_words(now)).kid(self.now_controls(app, now, grid_w)));
            }
            None => {
                body = body.kid(text(&v.idle, centred(ts(t::T_PHONE_HINT, Font::Sans, w(t::W70), 3))).fill_w()
                    .pad_xy(t::SPACE_12, t::SPACE_32));
            }
        }
        el().cover()
            .kid(super::shell::scroller("page-your-tv").kid(col().fill_w().no_shrink().items_center().pad4(gutter, HEAD + t::SPACE_8, gutter, t::SPACE_40).kid(body)))
            .kid(page_head(app, "your-tv-head", Some(v.title.as_str()), pos > t::K_PHONE_HEAD_SLOP, None))
    }

    /// The TV: its name, whether zunflix is open on it, and Play on TV; under them its own business, Change TV and Sign
    /// out TV.
    fn tv_tile(&self, app: &App) -> El {
        let v = self.your_tv.view();
        let top = row().fill_w().items_center()
            .kid(col().size(t::K_YOUR_TV_TILE_BOX, t::K_YOUR_TV_TILE_BOX).radius(t::K_YOUR_TV_TILE_CORNER).bg(ds::surface(2)).center().no_shrink()
                .kid(icon("tv", t::K_YOUR_TV_ICON, t::C_WHITE)))
            .kid(col().grow().margin(t::SPACE_14, 0., t::SPACE_10, 0.)
                .kid(line(&v.name, t::T_PHONE_TV_NAME, Font::Bold, t::C_WHITE))
                .kid(line(&v.status, t::T_NOTE, Font::Sans, w(t::W50)).margin(0., t::SPACE_2, 0., 0.)))
            .kid(line(&v.switch, t::T_PHONE_LABEL, Font::Bold, t::C_WHITE).margin(0., 0., t::SPACE_10, 0.))
            .kid(ds::switch(app, "tv-on", v.on, |a| a.pages.your_tv.send(Event::Switch)));
        let mut card = col().key("tv-tile").fill_w().pad(t::SPACE_16).radius(ds::R_LG).bg(ds::surface(1))
            .kid(top)
            .kid(el().fill_w().h(t::S_HAIRLINE).bg(t::C_LINE).margin(0., t::SPACE_14, 0., t::SPACE_14))
            .kid(row().fill_w().items_center()
                .kid(ds::button(app, "tv-change", ds::Kind::Secondary, ds::Size::S, Some("arrow_left_right"), &v.change, |a| a.pages.open_sheet(Sheet::Tv)))
                .kid(el().grow())
                .kid(ds::danger(app, "tv-sign-out", ds::Size::S, Some("log_out"), &v.sign_out, &v.sure_words, v.sure, |a| a.pages.your_tv.send(Event::SignOutTv))));
        if let Some(note) = &v.note {
            card = card.kid(ds::error_text(note, false).fill_w().margin(0., t::SPACE_10, 0., 0.));
        }
        card
    }

    /// How far along, the buttons, and the TV's choices and volume, `width` wide.
    fn now_controls(&self, app: &App, now: &Playing, width: f32) -> El {
        let place = now.place(net::now_ms() as f64 / 1000., self.tv_scrub);
        let dragging = self.tv_scrub.is_some();
        let (thick, knob) = if dragging { (t::S_SCRUB_DRAG, t::S_KNOB_DRAG) } else { (t::S_SCRUB, t::S_KNOB) };
        let touch = t::K_YOUR_TV_SCRUB_TOUCH;
        // (the played part in the accent; dragged, or clicked, it moves there)
        let knob_x = (width * place.share - knob / 2.).clamp(0., (width - knob).max(0.));
        let scrub = el().key("tv-scrub").w(width).h(touch).margin(0., t::SPACE_16, 0., 0.)
            .drag(|a, fraction, done| {
                let share = fraction.clamp(0., 1.);
                if done {
                    a.pages.tv_scrub = None;
                    a.pages.your_tv.send(Event::Scrub { share });
                } else {
                    a.pages.tv_scrub = Some(share);
                }
                a.kit.dirty = true;
            })
            .kid(el().at(0., (touch - thick) / 2., 0., f32::NAN).h(thick).radius(thick / 2.).clip().bg(w(t::W20)).decorative()
                .kid(el().at(0., 0., f32::NAN, 0.).pct_w(place.share).bg(crate::theme::accent())))
            .kid(el().at(knob_x, (touch - knob) / 2., f32::NAN, f32::NAN).size(knob, knob).radius(knob / 2.).bg(crate::theme::accent()).decorative());
        let times = row().fill_w().margin(0., t::SPACE_6, 0., 0.)
            .kid(line(&place.at, t::T_NOTE, Font::Medium, w(t::W60)))
            .kid(el().grow())
            .kid(line(&place.left, t::T_NOTE, Font::Medium, w(t::W60)));
        let play = tap(
            col().key("tv-play").size(t::K_YOUR_TV_PLAY, t::K_YOUR_TV_PLAY).radius(t::K_YOUR_TV_PLAY / 2.).bg(t::C_WHITE).center()
                .kid(icon(if now.paused { "play" } else { "pause" }, t::K_YOUR_TV_PLAY_ICON, t::C_BLACK).margin(if now.paused { t::SPACE_2 } else { 0. }, 0., 0., 0.)),
            t::PRESS_ROUND,
            |a| a.pages.your_tv.send(Event::Toggle),
        );
        let buttons = row().fill_w().center().gap(t::SPACE_32).margin(0., t::SPACE_14, 0., 0.)
            .kid(round_button(app, "tv-back10", "rotate_ccw", |a| a.pages.your_tv.send(Event::Jump { forward: false })))
            .kid(play)
            .kid(round_button(app, "tv-fwd10", "rotate_cw", |a| a.pages.your_tv.send(Event::Jump { forward: true })));
        let mut c = col().w(width).no_shrink().kid(scrub).kid(times).kid(buttons);
        if let Some(next) = &now.next {
            c = c.kid(ds::button(app, "tv-next", ds::Kind::Secondary, ds::Size::M, Some("skip_forward"), next, |a| a.pages.your_tv.send(Event::Next))
                .fill_w().margin(0., t::SPACE_16, 0., 0.));
        }
        let key = |id: &str, glyph: &'static str, press: Key| {
            let hot = ds::hot(app, id);
            tap(
                col().key(id).grow().fill_h().center().bg(if hot { w(t::W07) } else { 0 }).ease(ds::HOVER_MS)
                    .kid(icon(glyph, t::K_YOUR_TV_KEY_ICON, w(t::W80))),
                t::PRESS_ROUND,
                move |a| a.pages.your_tv.send(Event::Key { key: press }),
            )
        };
        let volume = row().key("tv-volume").grow().fill_h().radius(t::K_YOUR_TV_ROUND / 2.).clip().bg(ds::surface(2))
            .kid(key("tv-quieter", "volume_1", Key::VolumeDown))
            .kid(el().w(t::S_HAIRLINE).h(t::K_YOUR_TV_ROUND - t::SPACE_14 * 2.).margin(0., t::SPACE_14, 0., 0.).bg(t::C_LINE).no_shrink())
            .kid(key("tv-louder", "volume_2", Key::VolumeUp));
        let v = self.your_tv.view();
        c = c.kid(row().fill_w().h(t::K_YOUR_TV_ROUND).gap(t::SPACE_10).margin(0., t::SPACE_16, 0., 0.)
            .maybe((!v.captions.is_empty()).then(|| round_button(app, "tv-captions", "captions", |a| a.pages.open_sheet(Sheet::TvCaptions))))
            .kid(volume)
            .maybe((!v.settings.is_empty()).then(|| round_button(app, "tv-settings", "settings", |a| a.pages.open_sheet(Sheet::TvSettings)))));
        let stop_hot = ds::hot(app, "tv-stop");
        c.kid(tap(
            row().key("tv-stop").fill_w().h(ds::Size::M.h()).margin(0., t::SPACE_10, 0., 0.).center()
                .kid(line(&self.your_tv.view().stop, t::T_PHONE_LABEL, Font::Sans, w(if stop_hot { t::W80 } else { t::W50 }))),
            t::PRESS_CHIP,
            |a| a.pages.your_tv.send(Event::Stop),
        ))
    }

    /// The TV player's choices (its subtitles, or its sound and quality), picked from here.
    pub(super) fn tv_options_sheet(&self, app: &App, captions: bool) -> El {
        let v = self.your_tv.view();
        let groups = if captions { &v.captions } else { &v.settings };
        let mut c = col().fill_w();
        for g in groups {
            c = c.kid(line(&g.title, t::T_PHONE_GROUP, Font::Bold, t::C_WHITE).margin(0., t::SPACE_6, 0., t::SPACE_10));
            let mut chips = row().fill_w().wrap().gap(t::SPACE_6).margin(0., 0., 0., t::SPACE_14);
            for choice in &g.choices {
                let (group, label) = (g.title.clone(), choice.label.clone());
                chips = chips.kid(ds::chip(app, &format!("tv-pick-{}-{}", g.title, choice.label), &choice.label, choice.on, move |a| {
                    a.pages.your_tv.send(Event::Pick { group: group.clone(), choice: label.clone() });
                }));
            }
            c = c.kid(chips);
        }
        c.kid(ds::button(app, "tv-options-done", ds::Kind::Primary, ds::Size::M, None, &v.done, |a| a.pages.close_sheet()).fill_w())
    }

    /// Playing on the TV, over every page while Play on TV is on: what's on there and play/pause; a click opens Your TV.
    pub(super) fn tv_bar(&self, app: &App) -> Option<El> {
        let bar = self.your_tv.view().bar.as_ref()?;
        if matches!(self.route(), Some(Route::YourTv)) {
            return None;
        }
        let width = (view_w(app) - gutter(view_w(app)) * 2.).min(t::L_COMPACT_TEXT_MAX);
        let mut card = row().key("tv-bar").w(width).h(t::K_YOUR_TV_BAR).pad(t::SPACE_6).radius(t::K_YOUR_TV_BAR_CORNER).bg(t::C_RAISED).items_center()
            .kid(picture(el().size(t::K_YOUR_TV_BAR_ART_W, t::K_YOUR_TV_BAR_ART_H).no_shrink(), bar.art.as_deref().unwrap_or_default(), t::K_CARD_LANDSCAPE_RADIUS))
            .kid(col().grow().margin(t::SPACE_12, 0., t::SPACE_12, 0.)
                .kid(line(&bar.title, t::T_PHONE_TEXT, Font::Medium, t::C_WHITE))
                .kid(line(&bar.line, t::T_META, Font::Sans, t::C_INK3)));
        if let Some(paused) = bar.paused {
            card = card.kid(tap(
                col().key("tv-bar-play").size(t::K_YOUR_TV_BAR_BUTTON, t::K_YOUR_TV_BAR_BUTTON).radius(t::K_YOUR_TV_BAR_BUTTON / 2.).center()
                    .bg(if ds::hot(app, "tv-bar-play") { w(t::W07) } else { 0 })
                    .kid(icon(if paused { "play" } else { "pause" }, t::K_YOUR_TV_ICON, t::C_WHITE)),
                t::PRESS_ROUND,
                |a| a.pages.your_tv.send(Event::Toggle),
            ));
        }
        let card = tap(card, t::PRESS_ROW, |a| a.pages.open_your_tv());
        Some(row().key("tv-bar-layer").at(STAGE_X, f32::NAN, 0., t::SPACE_16).center().kid(card).anim(Anim::Rise(t::SPACE_10, t::D_MOVE as i64)))
    }

    /// The keys on the Your TV page: Space plays or pauses, Left and Right go back and forward 10, Up and Down set the
    /// TV's volume.
    pub fn your_tv_keydown(app: &mut App, vk: u16) -> bool {
        use crate::platform::key;
        if app.player.is_some() || app.pages.sheet().is_some() || app.pages.menu() || app.pages.route() != Some(&Route::YourTv) {
            return false;
        }
        let event = match vk {
            key::SPACE => Event::Toggle,
            key::LEFT => Event::Jump { forward: false },
            key::RIGHT => Event::Jump { forward: true },
            key::UP => Event::Key { key: Key::VolumeUp },
            key::DOWN => Event::Key { key: Key::VolumeDown },
            _ => return false,
        };
        app.pages.your_tv.send(event);
        app.kit.dirty = true;
        true
    }
}

/// What's on: its picture at 16:9, a spinner over it while the TV waits.
fn now_art(width: f32, now: &Playing) -> El {
    let mut art = picture(el().key("tv-art").w(width).h(width * t::K_CARD_WIDE_RATIO).no_shrink(), now.art.as_deref().unwrap_or_default(), t::R_STILL);
    if now.buffering {
        art = art.kid(col().cover().center().decorative().bg(black(t::W40)).kid(spinner(t::S_SPINNER, t::C_INK)));
    }
    art
}

/// What's on: its title, and where it is (or what went wrong).
fn now_words(now: &Playing) -> El {
    col().fill_w().margin(0., t::SPACE_16, 0., 0.)
        .kid(text(&now.title, ts(t::T_PHONE_NOW_TITLE, Font::Bold, t::C_WHITE, 2)).fill_w())
        .kid(text(&now.line, ts(t::T_PHONE_TEXT, Font::Sans, if now.failed { ds::DANGER } else { w(t::W70) }, 2)).fill_w().margin(0., t::SPACE_4, 0., 0.))
}

/// A round button on surface 2 with its icon, a light under the pointer.
fn round_button(app: &App, key: &str, glyph: &'static str, f: impl Fn(&mut App) + 'static) -> El {
    let size = t::K_YOUR_TV_ROUND;
    tap(
        col().key(key).size(size, size).radius(size / 2.).no_shrink().center().bg(if ds::hot(app, key) { ds::surface(3) } else { ds::surface(2) })
            .kid(icon(glyph, (size * t::K_YOUR_TV_ROUND_ICON_SHARE).round(), t::C_WHITE)),
        t::PRESS_ROUND,
        f,
    )
}
