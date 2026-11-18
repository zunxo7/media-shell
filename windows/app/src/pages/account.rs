//! The account, Downloads and Watch history, as the phone's: settings-like pages of glass rows; Downloads' and the
//! history's own bar. On a laptop their rows go across the width in columns, not one thin list with space beside it.
use super::shell::{gutter, page_head, HEAD};
use super::*;
use crate::ds::{choice, list, list_row, setting_line, switch, tile, Kind, Size};
use zcore::history::Row as HistoryRow;
use zcore::queue::{Entry, EntryKind, Item as Download};
use zcore::screen::Effect;
use zcore::settings::{self, Part};

/// On a laptop, Downloads and the history are grids of cards at least this wide (a narrow window keeps the phone's rows).
const CARD_MIN: f32 = t::K_ACCOUNT_CARD_MIN;

impl Pages {
    /// Settings: one scrolling page of tiles (a step lighter than the page, no outlines), in the order and with the
    /// words the core's page gives (`zcore::settings`): two columns, each stacking its own at their natural height,
    /// on a wide window; one on a narrower one. Every change is this profile's, saved at once.
    pub(super) fn account_view(&self, app: &App) -> El {
        let v = self.settings.view();
        let width = view_w(app);
        let gutter = gutter(width);
        let scroll_key = "page-account";
        let pos = app.kit.scroll_of(k(scroll_key)).pos;
        let grid_w = (width - gutter * 2.).min(t::L_LARGE_CONTENT_MAX);
        let gap = t::SPACE_16;
        let tiles = |parts: &[Part], w_: f32| -> Vec<El> { parts.iter().filter_map(|part| self.settings_tile(app, *part, w_)).collect() };
        // (rows that stretched tiles to one height measured some tiles wrong and let them overlap)
        let mut grid = if grid_w >= t::K_SETTINGS_TWO_COLUMNS_MIN {
            // (the left column as wide as the profiles need, never leaving room beside them; the right one takes the rest)
            let profiles = app.session.view().account.as_ref().map(zcore::profiles::tile);
            let main = profiles.as_ref().is_some_and(|t| t.manages);
            let cards = profiles.as_ref().map_or(1, |t| t.cards.len() + usize::from(t.add.is_some())) as f32;
            let hug = if main { cards * t::K_SETTINGS_PROFILE_HUG + (cards - 1.) * t::SPACE_12 + 2. * t::K_SETTINGS_TILE_PAD } else { t::K_SETTINGS_LEFT_COLUMN_MIN };
            let left_w = hug.max(t::K_SETTINGS_LEFT_COLUMN_MIN).min(grid_w - gap - t::K_SETTINGS_RIGHT_COLUMN_MIN);
            let right_w = grid_w - gap - left_w;
            let left = col().w(left_w).no_shrink().gap(gap).items_start().kids(tiles(&v.left, left_w));
            let right = col().w(right_w).no_shrink().gap(gap).items_start().kids(tiles(&v.right, right_w));
            col().w(grid_w).no_shrink().gap(gap).kid(row().w(grid_w).gap(gap).items_start().kid(left).kid(right))
        } else {
            col().w(grid_w).no_shrink().gap(gap).items_start().kids(tiles(&v.single, grid_w))
        };
        if !self.note.is_empty() {
            grid = grid.kid(ds::error_text(&self.note, false).fill_w());
        }
        let saved = v.saved.is_some();
        let tick = ds::badge(Some("check"), settings::SAVED).key("settings-saved")
            .opacity(if saved { 1. } else { 0. }).ease(if saved { 160 } else { 500 });
        el().cover()
            .kid(super::shell::scroller(scroll_key).kid(col().fill_w().no_shrink().items_center().pad4(gutter, HEAD + t::SPACE_8, gutter, t::SPACE_48).kid(grid)))
            .kid(page_head(app, "settings-head", Some(v.title.as_str()), pos > 4., Some(tick)))
    }

    fn settings_tile(&self, app: &App, part: Part, width: f32) -> Option<El> {
        let v = self.settings.view();
        Some(match part {
            Part::Profiles => self.tile_profile(app, width),
            Part::Subtitles => self.tile_subtitles(app, &v.subtitles, width),
            Part::Playback => self.tile_playback(app, &v.playback, width),
            Part::Downloads => self.tile_downloads(app, v.downloads.as_ref()?, width),
            Part::Languages => self.tile_languages(app, &v.languages, width),
            Part::Devices => self.tile_devices(app, v.devices.as_ref()?, width),
            Part::Version => self.version_card(app, width),
        })
    }

    /// Profiles (`zcore::profiles::tile`): the main profile's every profile as a card, each opening its small edit
    /// card, and Add; any other profile's own name and picture, edited right here.
    fn tile_profile(&self, app: &App, width: f32) -> El {
        let Some(profiles) = app.session.view().account.as_ref().map(zcore::profiles::tile) else { return el() };
        let main = profiles.manages;
        let inner = (if main { width } else { width.min(t::K_SETTINGS_LEFT_COLUMN_MIN) }) - 2. * t::K_SETTINGS_TILE_PAD;
        // a profile as a card: its picture, its name, Edit (the small card: name, picture, delete)
        let card = |c: &zcore::profiles::Card, w_: f32| {
            let p = data::me_of(&c.profile);
            let key = format!("set-prof-{}", p.id);
            let hot = app.kit.hover_within(k(&key));
            tap(
                col().key(&key).w(w_).pad(t::SPACE_14).radius(t::K_SETTINGS_CARD_RADIUS).items_center().bg(if hot { w(t::W09) } else { w(t::W05) })
                    .kid(avatar(Some(&p), (w_ - 2. * t::SPACE_14).min(t::K_SETTINGS_PROFILE_PICTURE), t::K_SETTINGS_PICTURE_RADIUS))
                    .kid(text(&p.name, centred(ts(t::T_ENTRY_NAME, Font::Bold, t::C_WHITE, 1))).fill_w().margin(0., t::SPACE_12, 0., 0.))
                    .kid(text(&c.role, centred(ts(t::T_META, Font::Medium, w(t::W50), 1))).fill_w().margin(0., t::SPACE_2, 0., 0.))
                    .kid(row().margin(0., t::SPACE_12, 0., 0.).h(t::K_CONTROL_BUTTON_S).pad_xy(t::SPACE_14, 0.).radius(t::K_CONTROL_BUTTON_S / 2.).items_center().gap(t::SPACE_6).bg(if hot { t::C_WHITE } else { w(t::W10) })
                        .kid(icon("pencil", t::K_SETTINGS_PENCIL, if hot { t::C_BLACK } else { t::C_WHITE })).kid(line(&profiles.edit, t::T_SETTINGS_CHOICE, Font::Bold, if hot { t::C_BLACK } else { t::C_WHITE }))),
                t::PRESS_DEFAULT,
                move |a| Pages::edit_profile(a, Some(p.clone())),
            )
        };
        let body = if main {
            // every profile on the account, and Add
            // (every card on one line, sharing the tile's width between them)
            let per_row = (profiles.cards.len() + usize::from(profiles.add.is_some())).max(1);
            // (cards at their own size; the tile hugs them, below)
            let w_ = t::K_SETTINGS_PROFILE_CARD_MAX.min((inner - t::SPACE_12 * (per_row as f32 - 1.)) / per_row as f32);
            let mut cards: Vec<El> = profiles.cards.iter().map(|c| card(c, w_)).collect();
            if let Some(most) = &profiles.add {
                let hot = app.kit.hover_within(k("set-prof-add"));
                cards.push(tap(
                    col().key("set-prof-add").w(w_).pad(t::SPACE_14).radius(t::K_SETTINGS_CARD_RADIUS).items_center().bg(if hot { w(t::W09) } else { w(t::W03) })
                        .kid(col().size((w_ - 2. * t::SPACE_14).min(t::K_SETTINGS_PROFILE_PICTURE), (w_ - 2. * t::SPACE_14).min(t::K_SETTINGS_PROFILE_PICTURE)).radius(t::K_SETTINGS_PICTURE_RADIUS).bg(w(t::W06)).center().kid(icon("plus", t::K_SETTINGS_ADD_GLYPH, w(t::W80))))
                        .kid(text(&profiles.add_title, centred(ts(t::T_ENTRY_NAME, Font::Bold, t::C_WHITE, 1))).fill_w().margin(0., t::SPACE_12, 0., 0.))
                        .maybe((!most.is_empty()).then(|| text(most, centred(ts(t::T_META, Font::Medium, w(t::W50), 1))).fill_w().margin(0., t::SPACE_2, 0., 0.))),
                    t::PRESS_DEFAULT,
                    |a| Pages::edit_profile(a, None),
                ));
            }
            let mut grid = col().fill_w().gap(t::SPACE_12);
            let mut it = cards.into_iter().peekable();
            while it.peek().is_some() {
                // (every card in a line as tall as the tallest: Add profile as tall as a profile with its Edit)
                grid = grid.kid(row().fill_w().gap(t::SPACE_12).items_stretch().kids(it.by_ref().take(per_row).collect()));
            }
            grid
        } else {
            // just yourself, edited right here (no pop-up): the picture (a click picks another), the name, Save
            let edit = self.editor.view();
            let shown = self.me.as_ref().map(|m| Me { avatar: if edit.removed { None } else { m.avatar.clone() }, ..m.clone() });
            let picture = match &self.avatar {
                Some(path) => col().size(t::K_SETTINGS_SHEET_PICTURE, t::K_SETTINGS_SHEET_PICTURE).radius(t::K_SETTINGS_PICTURE_RADIUS).clip().bg(t::C_BG).image(format!("file:{path}")),
                None => avatar(shown.as_ref(), t::K_SETTINGS_SHEET_PICTURE, t::K_SETTINGS_PICTURE_RADIUS),
            };
            let pick_hot = app.kit.hover_within(k("set-pick-picture"));
            // (a picture just picked: in its cropper, moved and zoomed to fit; else the picture, a click picks one)
            let pick = if self.avatar_pending {
                crate::ui::skeleton(el().size(t::K_SETTINGS_SHEET_PICTURE, t::K_SETTINGS_SHEET_PICTURE).no_shrink(), t::K_SETTINGS_PICTURE_RADIUS)
            } else if self.avatar.is_some() {
                col().no_shrink().items_center().gap(t::SPACE_6)
                    .kid(self.crop_editor(app, t::K_SETTINGS_INLINE_CROP_FRAME, t::K_SETTINGS_INLINE_CROP_RADIUS))
                    .kid(ds::button(app, "set-choose-another", Kind::Ghost, Size::S, None, &edit.words.choose_another, Pages::change_avatar))
            } else {
                tap(
                    el().key("set-pick-picture").size(t::K_SETTINGS_SHEET_PICTURE, t::K_SETTINGS_SHEET_PICTURE).no_shrink()
                        .kid(picture)
                        .kid(col().cover().decorative().radius(t::K_SETTINGS_PICTURE_RADIUS).bg(black(0.45)).center().kid(icon("pencil", t::K_SETTINGS_PICK_PENCIL, t::C_WHITE))
                            .opacity(if pick_hot { 1. } else { 0. }).ease(200)),
                    t::PRESS_DEFAULT,
                    Pages::change_avatar,
                )
            };
            let save: Cb = Rc::new(Pages::save_profile);
            let label = if self.saved_at.is_some() { "Saved" } else { edit.button.as_str() };
            let field_w = (inner - t::K_SETTINGS_SHEET_PICTURE - t::K_SETTINGS_FORM_GAP - t::K_SETTINGS_SAVE_ROOM).max(t::K_SETTINGS_FIELD_MIN);
            let form = col().grow().gap(t::SPACE_10)
                .kid(line(&edit.words.name, t::T_FIELD_LABEL, Font::Bold, ds::text2()))
                .kid(row().fill_w().items_center().gap(t::SPACE_10)
                    .kid(glass_field(app, Field { key: "profile-name", hint: "Your name", submit: Some(save.clone()), ..Default::default() }, field_w, None))
                    .kid(ds::button(app, "set-save-profile", Kind::Primary, Size::L, None, label, move |a| save(a))));
            let mut body = row().fill_w().items_center().gap(t::K_SETTINGS_FORM_GAP).kid(pick).kid(form);
            if let Some(note) = &edit.note {
                body = body.kid(ds::error_text(note, false));
            }
            body
        };
        tile("user_round", &profiles.title, &profiles.line, width, body)
    }

    /// Subtitles: a live preview (a still with a line drawn exactly as the player will), then on, size, style and the
    /// box behind them.
    fn tile_subtitles(&self, app: &App, tw: &settings::Subtitles, width: f32) -> El {
        let inner = width - 2. * t::K_SETTINGS_TILE_PAD;
        // (compact: at most ~220 tall and ~42% of the tile; beside the controls whenever four options still fit on a
        // line next to it, else centred above them at the same capped size)
        const MAX_W: f32 = t::K_SETTINGS_PREVIEW_MAX_H / t::K_CARD_WIDE_RATIO;
        const CONTROLS: f32 = t::K_SETTINGS_CAPTION_CONTROLS_MIN;
        let beside_w = (inner - t::K_SETTINGS_PREVIEW_GAP - CONTROLS).min(inner * t::K_SETTINGS_PREVIEW_MAX_SHARE).min(MAX_W);
        let side = beside_w >= t::K_SETTINGS_PREVIEW_BESIDE_MIN;
        let preview_w = if side { beside_w } else { inner.min(MAX_W) };
        let preview_h = preview_w * t::K_CARD_WIDE_RATIO;
        let on = crate::profile_settings::cc_on();
        let size_ = crate::profile_settings::cc_size();
        let style = crate::profile_settings::cc_color();
        let background = crate::profile_settings::cc_background();
        // (a neutral studio backdrop, as caption previews have: the styles shown plainly, not over a busy picture)
        let still = "res:caption_preview".to_string();
        // (an exact miniature: what the player draws full-window at this window's size, all of it shrunk alike)
        // (a close-up of the screen's lower middle, the miniature doubled: the sizes read, still in proportion)
        let shrink = 2. * preview_h / app.kit.height.max(1.);
        let scale = crate::player::caption_size(&size_, app.kit.height) / t::T_CAPTION.0;
        let mut preview = picture(el().key("set-cc-preview").size(preview_w, preview_h).no_shrink(), &still, t::K_SETTINGS_PREVIEW_RADIUS).bg(t::C_STILL_GROUND);
        if on {
            preview = preview.kid(crate::player::caption_lines_as(&tw.preview, t::T_CAPTION.0 * scale, &style, background, shrink)
                .at(t::K_PLAYER_WORDS_SIDE * shrink, f32::NAN, t::K_PLAYER_WORDS_SIDE * shrink, t::K_PLAYER_HIDDEN_FOOT * shrink));
        }
        let shade = zcore::captions::shade(background.into());
        let controls = col().grow().gap(t::K_SETTINGS_CONTROLS_GAP)
            .kid(setting_line(&tw.on.name, Some(tw.on.line.as_str()), switch(app, "set-cc-on", on, { let (g, k_) = (tw.on.group.clone(), tw.on.key.clone()); move |a: &mut App| { let v = !crate::profile_settings::cc_on(); save(a, &g, &k_, json!(v)) } })))
            .kid(pick(app, "set-cc-size", &tw.size, &size_, |v| json!(v)))
            .kid(pick(app, "set-cc-color", &tw.colour, &style, |v| json!(v)))
            // (no styles: a colour, a size and a background make every one of them)
            .kid(pick(app, "set-cc-bg", &tw.background, shade, |v| json!(zcore::captions::shade_value(v))));
        let body = if side {
            row().fill_w().items_start().gap(t::K_SETTINGS_PREVIEW_GAP).kid(preview).kid(controls)
        } else {
            col().fill_w().gap(t::SPACE_20).kid(col().fill_w().items_center().kid(preview)).kid(controls)
        };
        tile("captions", &tw.head.title, &tw.head.line, width, body)
    }

    /// Playback: the next episode by itself, and the quality it streams at.
    fn tile_playback(&self, app: &App, tw: &settings::Playback, width: f32) -> El {
        // (the player's Quality saves levels Settings doesn't list, "1440p", "1080p HDR": shown as the nearest here)
        let quality = zcore::quality::shown_choice(&crate::profile_settings::quality(), crate::data::plus());
        let auto = crate::profile_settings::autoplay_next();
        let (g, k_) = (tw.autoplay.group.clone(), tw.autoplay.key.clone());
        let body = col().fill_w().gap(t::SPACE_20)
            .kid(setting_line(&tw.autoplay.name, Some(tw.autoplay.line.as_str()), switch(app, "set-autoplay", auto, move |a| { let v = !crate::profile_settings::autoplay_next(); save(a, &g, &k_, json!(v)) })))
            .kid(pick(app, "set-quality", &tw.quality, quality, |v| json!(v)));
        tile("play", &tw.head.title, &tw.head.line, width, body)
    }

    /// Downloads: the quality they're saved in.
    fn tile_downloads(&self, app: &App, tw: &settings::Downloads, width: f32) -> El {
        let tier = crate::profile_settings::download_tier();
        let body = col().fill_w().gap(t::SPACE_20).kid(pick(app, "set-dl", &tw.quality, &tier, |v| json!(v)));
        tile("download", &tw.head.title, &tw.head.line, width, body)
    }

    /// Languages: anime's audio, and dramas from abroad.
    fn tile_languages(&self, app: &App, tw: &settings::Languages, width: f32) -> El {
        let audio = crate::profile_settings::anime_audio();
        let dramas = crate::profile_settings::dramas_audio();
        let body = col().fill_w().gap(t::SPACE_20)
            .kid(pick(app, "set-anime", &tw.anime, &audio, |v| json!(v)))
            .kid(pick(app, "set-dramas", &tw.dramas, &dramas, |v| json!(v)));
        tile("languages", &tw.head.title, &tw.head.line, width, body)
    }

    /// Devices: every place this account is signed in.
    fn tile_devices(&self, app: &App, tw: &settings::Devices, width: f32) -> El {
        let mut rows = col().fill_w();
        if tw.loading {
            rows = rows.kid(skeleton(el().fill_w().h(t::K_SETTINGS_LIST_ROW), 0.));
        }
        for (i, d) in tw.rows.iter().enumerate() {
            if i > 0 { rows = rows.kid(ds::divider()); }
            let glyph = match d.kind { settings::DeviceKind::Tv => "tv", settings::DeviceKind::Computer => "laptop", settings::DeviceKind::Phone => "smartphone" };
            let end = d.remove.as_ref().map(|r| {
                let id = d.id.clone();
                ds::danger(app, &format!("remove-{id}"), Size::S, None, &r.label, &r.confirm, r.asking, move |a| a.pages.settings.send(settings::Event::Remove { id: id.clone() }))
            });
            rows = rows.kid(list_row(app, &format!("device-{}", d.id), ds::lead_icon(glyph), &d.name, Some(d.line.clone()), None, end));
        }
        let mut body = col().fill_w().gap(t::SPACE_14);
        if let Some(count) = &tw.count {
            body = body.kid(line(count, t::T_SUBHEAD, Font::Bold, ds::text3()).margin(t::SPACE_4, 0., 0., 0.));
        }
        body = body.kid(list(rows));
        // signing out every other device: a plain button; red only for its "click again" (signing out of this PC is in
        // the profile menu)
        if let Some(o) = &tw.others {
            let others = |a: &mut App| a.pages.settings.send(settings::Event::RemoveOthers);
            body = body.kid(if o.asking {
                ds::danger(app, "set-others", Size::M, Some("log_out"), "", &o.confirm, true, others)
            } else {
                ds::button(app, "set-others", Kind::Secondary, Size::M, Some("log_out"), &o.label, others)
            });
        }
        if let Some(note) = &tw.note {
            body = body.kid(ds::error_text(note, false).fill_w());
        }
        tile("laptop", &tw.head.title, &tw.head.line, width, body)
    }

    /// The app's version, compactly, closing Settings: its mark, name and version; up to date, or a newer one to
    /// update to now (the bootstrapper, over the splash, as when it opens), as the core's updater says.
    fn version_card(&self, app: &App, width: f32) -> El {
        let card = &app.updater.view().version;
        // (the app's own icon, as the taskbar and the bootstrapper show it)
        let mark = el().size(t::K_SETTINGS_APP_ICON, t::K_SETTINGS_APP_ICON).no_shrink().image("res:icon");
        let state: El = match &card.offer {
            Some(offer) => {
                // (the new version named under the pointer, in a little label over the button: no line of its own)
                let button = ds::button(app, "version-update", Kind::Primary, Size::S, Some("download"), &offer.button, |a| a.updater.send(zcore::update::Event::Update));
                let tip = app.kit.hover_within(k("version-update")).then(|| {
                    row().at(f32::NAN, f32::NAN, 0., f32::NAN).margin(0., -t::K_SETTINGS_TOOLTIP_LIFT, 0., 0.).h(t::K_TOOLTIP_HEIGHT).pad_xy(t::K_TOOLTIP_PAD, 0.).radius(t::R_SMALL).items_center()
                        .bg(t::C_TOOLTIP).decorative().anim(Anim::Fade(140))
                        .kid(line(&offer.version, t::T_TOOLTIP, Font::Medium, t::C_WHITE).no_shrink())
                });
                el().kid(button).maybe(tip)
            }
            None => ds::badge(Some("check"), &card.up_to_date),
        };
        // (uninstalling: zunflix closes and its bootstrapper takes it off this PC, no asking first)
        let remove = ds::danger(app, "version-uninstall", Size::S, Some("trash"), "Uninstall", "", false, |a| {
            crate::platform::install::uninstall_from_app();
            a.window.close();
        });
        let state = row().items_center().gap(t::SPACE_8).kid(state).kid(remove);
        row().key("version-card").w(width).pad4(t::SPACE_16, t::SPACE_14, t::K_SETTINGS_VERSION_PAD_END, t::SPACE_14).radius(ds::R_LG).bg(ds::surface(1)).items_center().gap(t::SPACE_14)
            .kid(mark)
            .kid(col().grow()
                .kid(line(&card.name, t::T_ENTRY_NAME, Font::Bold, t::C_WHITE))
                .kid(line(&card.line, t::T_NOTE, Font::Medium, ds::text3()).margin(0., t::K_CONTROL_LINE_GAP, 0., 0.)))
            .kid(state)
    }

    pub(super) fn downloads_view(&self, app: &App) -> El {
        let width = view_w(app);
        let gutter = gutter(width);
        let pos = app.kit.scroll_of(k("page-downloads")).pos;
        let finding = self.finding == Some("downloads");
        let query = if finding { app.kit.text_of(&super::find_field("downloads")) } else { String::new() };
        downloads::search(&query);
        downloads::show(self.dl_show.clone());
        let v = downloads::view();
        let page = &v.page;
        // (all of them, or a show's episodes: each comes in as a page does)
        let view = page.show.as_ref().map_or("dl-all".to_string(), |id| format!("dl-show-{id}"));
        let mut body = col().key(&view).fill_w().no_shrink().pad4(gutter, HEAD, gutter, t::SPACE_40).anim(Anim::Page);
        if !page.room.is_empty() {
            body = body.kid(line(&page.room, t::T_ACCOUNT_COUNT, Font::Medium, t::C_MUTED).margin(0., 0., 0., t::SPACE_6));
        }
        if !self.note.is_empty() {
            body = body.kid(text(&self.note, ts(t::T_ERROR, Font::Sans, t::C_INK2, 2)).margin(0., 0., 0., t::SPACE_6));
        }
        if let Some(nothing) = &page.empty {
            let glyph = super::glyph_icon(nothing.glyph);
            body = body.kid(el().fill_w().h((app.kit.height - HEAD * 2. - t::SPACE_40).max(t::K_ACCOUNT_EMPTY_MIN)).center().kid(empty(glyph, &nothing.title, &nothing.line)));
        }
        let content = width - gutter * 2.;
        let (columns, cell) = columns(content);
        let lay_out = |body: El, cards: Vec<El>| -> El {
            let mut body = body;
            let mut cards = cards.into_iter().peekable();
            while cards.peek().is_some() {
                body = body.kid(row().fill_w().gap(t::SPACE_16).items_start().margin(0., t::SPACE_14, 0., t::SPACE_8).kids(cards.by_ref().take(columns).collect()));
            }
            body
        };
        let item = |e: &Entry| e.item.and_then(|i| v.items.get(i as usize));
        if page.show.is_some() {
            let cards = page.entries.iter().filter_map(item).enumerate()
                .map(|(i, d)| self.download_tile(app, d, &page.ask, cell, columns).anim(Anim::Rise(t::SPACE_14, 240 + 45 * i.min(10) as i64))).collect();
            body = lay_out(body, cards);
        } else {
            // every film and every show side by side across the width (a film as its card, a show as one stacked card
            // that opens on its episodes); then each manhwa under its name, its chapters across the width
            let videos = page.entries.iter().filter(|e| matches!(e.kind, EntryKind::Film | EntryKind::Show))
                .filter_map(|e| match e.kind {
                    EntryKind::Show => Some(self.show_card(app, e, &page.ask, cell)),
                    _ => item(e).map(|d| self.download_tile(app, d, &page.ask, cell, columns).margin(0., t::SPACE_14, 0., 0.)),
                })
                .collect();
            body = lay_out(body, videos);
            let mut chapters: Vec<El> = vec![];
            for e in page.entries.iter().filter(|e| matches!(e.kind, EntryKind::Series | EntryKind::Chapter)) {
                if e.kind == EntryKind::Series {
                    body = lay_out(body, std::mem::take(&mut chapters));
                    body = body.kid(line(&e.name, t::T_DAY_HEADING, Font::Bold, t::C_WHITE).margin(0., t::K_ACCOUNT_GROUP_GAP, 0., t::SPACE_2));
                } else if let Some(d) = item(e) {
                    chapters.push(self.download_tile(app, d, &page.ask, cell, columns));
                }
            }
            body = lay_out(body, chapters);
        }
        // (Settings, as the rail's Settings opens it: download quality is there, under Playback)
        let settings = ds::head_button(app, "dl-settings", "settings", Pages::open_settings);
        let head = match &page.show {
            // (a show open: Back to every download, its name beside it; nothing else)
            Some(_) => ds::head(app, "downloads-show-head", gutter, Some(Rc::new(|a: &mut App| a.pages.dl_show = None)), Some(&page.heading), pos > 4., None),
            None => finding_bar(self, app, "downloads", &page.heading, &page.search_hint, pos > 4., Some(settings), None),
        };
        el().cover()
            .kid(super::shell::scroller("page-downloads").kid(body))
            .kid(head)
    }

    fn download_tile(&self, app: &App, d: &Download, ask: &str, width: f32, columns: usize) -> El {
        if columns > 1 { self.download_card(app, d, ask, width) } else { self.download_row(app, d, ask, width) }
    }

    /// A show on Downloads: its picture as a card with two more stacked behind it (its episodes), its name, how many
    /// are down and how big they are; a click opens it on its episodes.
    fn show_card(&self, app: &App, show: &Entry, ask: &str, width: f32) -> El {
        let id = show.key.trim_start_matches("show:").to_string();
        let key = format!("ds-{id}");
        let h = width * t::K_CARD_WIDE_RATIO;
        let hot = app.kit.hover_within(k(&key));
        let url = sharp(show.picture.as_deref().unwrap_or_default());
        // (the two behind it peek out above, narrower and dimmer; under the pointer they fan up a little more)
        let rise = if hot { t::SPACE_4 } else { 0. };
        let stack = el().key(&key).w(width).h(h + t::SPACE_14).hover()
            .kid(el().at(width * (1. - t::K_ACCOUNT_STACK_BACK_SHARE) / 2., -rise, f32::NAN, f32::NAN).w(width * t::K_ACCOUNT_STACK_BACK_SHARE).h(h).radius(t::R_STILL).bg(w(t::W06)).slide(200).decorative())
            .kid(el().at(width * (1. - t::K_ACCOUNT_STACK_MIDDLE_SHARE) / 2., t::SPACE_14 / 2. - rise / 2., f32::NAN, f32::NAN).w(width * t::K_ACCOUNT_STACK_MIDDLE_SHARE).h(h).radius(t::R_STILL).bg(w(t::W11)).slide(200).decorative())
            .kid(picture(el().at(0., t::SPACE_14, f32::NAN, f32::NAN).w(width).h(h), &url, t::R_STILL)
                .kid(el().cover().decorative().bg(black(0.16)).opacity(if hot { 0. } else { 1. }).ease(280))
                .kid(row().at(f32::NAN, f32::NAN, t::SPACE_10, t::SPACE_10).pad4(t::SPACE_10, t::K_DOWNLOADS_BADGE_PAD_Y, t::SPACE_10, t::K_DOWNLOADS_BADGE_PAD_Y).radius(t::R_PILL).bg(black(0.6)).decorative()
                    .kid(line(show.badge.clone(), t::T_META, Font::Bold, t::C_WHITE))));
        let open = id.clone();
        // (the whole show deleted at once, asked once more, as a film is)
        let asking_key = show.key.clone();
        let keys: Vec<String> = downloads::items().into_iter().filter(|i| i.kind == downloads::Kind::Video && i.title_id == id).map(|i| i.key).collect();
        let asking = self.confirm == asking_key;
        let delete = ds::danger_icon(app, &format!("delete-{key}"), Size::M, "trash", ask, asking, move |a| {
            if a.pages.confirm == asking_key { keys.iter().for_each(|k| downloads::remove(k)); a.pages.confirm.clear(); } else { a.pages.confirm = asking_key.clone(); }
        });
        // (laid out as a film's card: its name and what it's doing, its control in the middle of the two)
        col().key(&format!("{key}-card")).w(width)
            .kid(tap(stack.grow_hover(1.03), t::PRESS_DEFAULT, move |a| {
                // (into a show: the search closes, as the phone's does)
                a.pages.close_find();
                a.pages.dl_show = Some(open.clone());
                a.kit.reset_scroll(k("page-downloads"));
            }))
            .kid(row().fill_w().margin(0., t::SPACE_10, 0., 0.).items_center()
                .kid(col().grow()
                    .kid(text(show.name.clone(), ts(t::T_ITEM_TITLE, Font::Bold, t::C_WHITE, 2)).fill_w())
                    .kid(text(show.line.clone(), ts(t::T_NOTE, Font::Sans, if show.failed { ds::DANGER } else { t::C_INK3 }, 1)).fill_w().margin(0., t::K_CONTROL_LINE_GAP, 0., 0.)))
                .kid(delete.margin(t::SPACE_6, 0., t::SPACE_4, 0.)))
    }

    /// A download's picture, and what opening it does (play it, read it) once it's down.
    fn download_parts(d: &Download) -> (String, crate::kit::Cb) {
        if d.kind == downloads::Kind::Chapter {
            let row = json!({"titleId": d.title_id, "title": d.title, "art": d.art, "cover": d.art, "key": d.key, "label": d.label, "thumb": d.picture});
            // (no still from the server nor a cover: one made here from its pages)
            let picture = d.picture.as_deref().map_or_else(|| crate::images::chapter_still(&d.key).unwrap_or_default(), net::absolute);
            return (picture, Rc::new(move |a: &mut App| a.pages.read_saved(&row)));
        }
        let item = d.clone();
        (sharp(d.picture.as_deref().unwrap_or_default()), Rc::new(move |a: &mut App| offline_play(a, &item)))
    }

    /// A download's control and delete: the one rule for its control, and delete asked once more.
    fn download_ends(&self, app: &App, d: &Download, ask: &str, open: crate::kit::Cb) -> (El, El) {
        let key = d.key.clone();
        // (done: nothing beside the name; the picture plays or reads it)
        let action = if d.state == downloads::State::Done { el() } else {
            download_control(app, &format!("act-{key}"), &key, Some((d.state, d.share)), Rc::new(|_: &mut App| {}), open)
        };
        let asking = self.confirm == key;
        let k2 = key.clone();
        let delete = ds::danger_icon(app, &format!("delete-{key}"), Size::M, "trash", ask, asking, move |a| {
            if a.pages.confirm == k2 { downloads::remove(&k2); a.pages.confirm.clear(); } else { a.pages.confirm = k2.clone(); }
        });
        (action, delete)
    }

    fn download_row(&self, app: &App, d: &Download, ask: &str, width: f32) -> El {
        let key = d.key.clone();
        let (url, open) = Self::download_parts(d);
        let still = picture(el().size(t::S_STILL_W, t::S_STILL_H).no_shrink(), &url, t::R_STILL);
        let (action, delete) = self.download_ends(app, d, ask, open.clone());
        let play = d.state == downloads::State::Done;
        tap(
            row().key(&format!("dl-{key}")).w(width).pad_xy(0., t::SPACE_10).items_center()
                .kid(still)
                .kid(col().grow().margin(t::SPACE_12, 0., t::SPACE_8, 0.)
                    .kid(text(d.name.clone(), ts(t::T_ITEM_TITLE, Font::Bold, t::C_WHITE, 2)).fill_w())
                    .kid(text(d.line.clone(), ts(t::T_NOTE, Font::Sans, if d.failed { ds::DANGER } else { t::C_INK3 }, if d.failed { 2 } else { 1 })).fill_w().margin(0., t::K_CONTROL_LINE_GAP, 0., 0.)))
                .kid(col().key(&format!("act-slot-{key}")).size(t::K_DOWNLOADS_SLOT, t::K_DOWNLOADS_SLOT).center().kid(action))
                .kid(delete.margin(t::SPACE_4, 0., 0., 0.)),
            t::PRESS_ROW,
            move |a| if play { open(a) },
        )
    }

    /// A download as a laptop shows it: the still across the card, the name, how it's going, and its controls beside
    /// the name.
    fn download_card(&self, app: &App, d: &Download, ask: &str, width: f32) -> El {
        let key = d.key.clone();
        let play = d.state == downloads::State::Done;
        let chapter = d.kind == downloads::Kind::Chapter;
        let (url, open) = Self::download_parts(d);
        let mut still = picture(el().w(width).h(width * t::K_CARD_WIDE_RATIO), &url, t::R_STILL);
        // (under the pointer, as Home's cards: brighter, ringed, Play (or Read) fading in, a little bigger; all easing
        // back out)
        let hot = app.kit.hover_within(k(&format!("dc-art-{key}")));
        still = still.kid(el().cover().decorative().bg(black(0.16)).opacity(if hot { 0. } else { 1. }).ease(280));
        if play {
            still = still.kid(col().cover().center().decorative().opacity(if hot { 1. } else { 0.75 }).ease(240)
                .kid(col().size(t::K_DOWNLOADS_PLAY, t::K_DOWNLOADS_PLAY).radius(t::K_DOWNLOADS_PLAY / 2.).bg(if hot { t::C_WHITE } else { black(0.45) }).center()
                    .kid(icon(if chapter { "book_open" } else { "play" }, t::K_DOWNLOADS_PLAY_ICON, if hot { t::C_BLACK } else { t::C_WHITE }))));
        }
        let (action, delete) = self.download_ends(app, d, ask, open.clone());
        col().key(&format!("dc-{key}")).w(width)
            .kid(tap(still.key(&format!("dc-art-{key}")).grow_hover(1.04), t::PRESS_DEFAULT, move |a| if play { open(a) }))
            // (its name and what it's doing, its controls in the middle of the two: a long name takes two lines, then
            // "…"; what it's doing one line (two for why it failed), so a change of words hardly moves them)
            .kid(row().fill_w().margin(0., t::SPACE_10, 0., 0.).items_center()
                .kid(col().grow()
                    .kid(text(d.name.clone(), ts(t::T_ITEM_TITLE, Font::Bold, t::C_WHITE, 2)).fill_w())
                    .kid(text(d.line.clone(), ts(t::T_NOTE, Font::Sans, if d.failed { ds::DANGER } else { t::C_INK3 }, if d.failed { 2 } else { 1 })).fill_w().margin(0., t::K_CONTROL_LINE_GAP, 0., 0.)))
                // (its control in a slot that keeps its place whatever it is: pause, resume and retry are different keys)
                .kid(el().key(&format!("act-slot-{key}")).kid(action))
                .kid(delete.margin(t::SPACE_6, 0., t::SPACE_4, 0.)))
    }

    /// Watch history, drawn from the core's view model (zcore::history).
    pub(super) fn history_view(&self, app: &App) -> El {
        let v = self.history.view();
        let width = view_w(app);
        let gutter = gutter(width);
        let pos = app.kit.scroll_of(k("page-history")).pos;
        let content = width - gutter * 2.;
        let (columns, cell) = columns(content);
        let mut body = col().fill_w().no_shrink().pad4(gutter, HEAD, gutter, t::SPACE_40);
        if let Some(count) = &v.count {
            body = body.kid(line(count, t::T_ACCOUNT_COUNT, Font::Medium, t::C_MUTED).margin(0., 0., 0., t::SPACE_6));
        }
        let middle = |inner: El| el().fill_w().h((app.kit.height - HEAD * 2. - t::SPACE_40).max(t::K_ACCOUNT_EMPTY_MIN)).center().kid(inner);
        if let Some(trouble) = &v.trouble {
            let retry = |a: &mut App| a.pages.history.send(zcore::history::Event::Retry);
            body = body.kid(middle(crate::ui::trouble("history-trouble", &trouble.message, trouble.offline, trouble.busy, retry, None)));
        } else if let Some(nothing) = &v.empty {
            let glyph = super::glyph_icon(nothing.glyph);
            body = body.kid(middle(empty(glyph, &nothing.title, &nothing.line)));
        } else if v.loading {
            body = body.kid(skeleton(el().size(t::K_ACCOUNT_COUNT_SKELETON_W, t::K_ACCOUNT_COUNT_SKELETON_H).margin(0., t::SPACE_14, 0., t::SPACE_6), t::R_SMALL));
            for _ in 0..4 {
                body = body.kid(row().fill_w().gap(t::SPACE_16).margin(0., t::SPACE_8, 0., t::SPACE_6).kids((0..columns).map(|_| {
                    if columns > 1 {
                        col().w(cell)
                            .kid(skeleton(el().w(cell).h(cell * t::K_CARD_WIDE_RATIO), t::R_STILL))
                            .kid(skeleton(el().pct_w(0.6).h(t::K_ACCOUNT_NAME_SKELETON).margin(0., t::SPACE_12, 0., 0.), t::K_ACCOUNT_NAME_SKELETON_RADIUS))
                            .kid(skeleton(el().pct_w(0.4).h(t::K_ACCOUNT_LINE_SKELETON).margin(0., t::K_ACCOUNT_SKELETON_GAP, 0., 0.), t::K_ACCOUNT_LINE_SKELETON_RADIUS))
                    } else {
                        row().w(cell).pad_xy(0., t::SPACE_10).items_center()
                            .kid(skeleton(el().size(t::S_STILL_W, t::S_STILL_H).no_shrink(), t::R_STILL))
                            .kid(col().grow().margin(t::SPACE_12, 0., 0., 0.).kid(skeleton(el().pct_w(0.6).h(t::K_ACCOUNT_NAME_SKELETON), t::K_ACCOUNT_NAME_SKELETON_RADIUS)).kid(skeleton(el().pct_w(0.4).h(t::K_ACCOUNT_LINE_SKELETON).margin(0., t::K_ACCOUNT_SKELETON_GAP, 0., 0.), t::K_ACCOUNT_LINE_SKELETON_RADIUS)))
                    }
                }).collect()));
            }
        }
        // each day's rows across the width
        for (i, day) in v.days.iter().enumerate() {
            body = body.kid(line(&day.heading, t::T_DAY_HEADING, Font::Bold, t::C_WHITE).margin(0., if i == 0 { t::SPACE_10 } else { t::K_ACCOUNT_GROUP_GAP }, 0., t::SPACE_2));
            for chunk in day.rows.chunks(columns) {
                body = body.kid(row().fill_w().gap(t::SPACE_16).items_start().margin(0., t::SPACE_8, 0., t::SPACE_6)
                    .kids(chunk.iter().map(|r| if columns > 1 { history_card(r, cell) } else { history_row(r, cell) }).collect()));
            }
        }
        let typed: Cb = Rc::new(|a: &mut App| {
            let text = a.kit.text_of(&super::find_field("history"));
            a.pages.history.send(zcore::history::Event::Query { text });
        });
        el().cover()
            .kid(super::shell::scroller("page-history").kid(body))
            .kid(finding_bar(self, app, "history", &v.title, &v.hint, pos > 4., None, Some(typed)))
    }
}

/// As many columns of cards as fit at 220 and up, gap 16, and how wide each is; a narrow window keeps the phone's rows.
fn columns(content: f32) -> (usize, f32) {
    if content < t::K_ACCOUNT_COLUMNS_MIN {
        return (1, content);
    }
    let n = ((content + t::SPACE_16) / (CARD_MIN + t::SPACE_16)).floor().max(2.) as usize;
    (n, (content - t::SPACE_16 * (n - 1) as f32) / n as f32)
}

/// The history as a laptop shows it: the still across the card (how far in along its foot), the name and line under it.
fn history_card(r: &HistoryRow, width: f32) -> El {
    let url = net::img(r.picture.as_deref().unwrap_or_default());
    let key = format!("hc-{}", r.key);
    // (under the pointer, as Home's cards: brighter, ringed, Play fading in, a little bigger; all easing back out,
    // played by the kit itself, nothing built again)
    let still = picture(el().w(width).h(width * t::K_CARD_WIDE_RATIO), &url, t::R_STILL)
        .kid(el().cover().decorative().bg(black(0.16)).fade_on_hover(k(&key), 1., 0.).ease(280))
        .kid(col().cover().center().decorative().fade_on_hover(k(&key), 0., 1.).ease(240)
            .kid(col().size(t::K_PAGE_PLAY_CIRCLE, t::K_PAGE_PLAY_CIRCLE).radius(t::K_PAGE_PLAY_CIRCLE / 2.).bg(t::C_WHITE).center().kid(icon("play", t::K_PAGE_PLAY_GLYPH, t::C_BLACK).margin(t::SPACE_2, 0., 0., 0.))))
        .maybe(r.progress.map(progress_bar));
    let tapped = r.key.clone();
    tap(
        // (keyed as its hover's fades follow it: under the pointer, they play)
        col().key(&key).w(width)
            .kid(still.grow_hover(1.04))
            .kid(text(&r.title, ts(t::T_ITEM_TITLE, Font::Bold, t::C_WHITE, 1)).fill_w().margin(0., t::SPACE_10, 0., 0.))
            .kid(text(&r.line, ts(t::T_NOTE, Font::Sans, w(t::W50), 1)).fill_w().margin(0., t::K_CONTROL_LINE_GAP, 0., 0.)),
        t::PRESS_DEFAULT,
        move |a| a.pages.history.send(zcore::history::Event::Tapped { key: tapped.clone() }),
    )
}

/// How far along, along a still's foot.
fn progress_bar(share: f32) -> El {
    el().at(0., f32::NAN, 0., 0.).h(t::S_PROGRESS).decorative().bg(w(t::W25)).kid(el().pct_w(share).fill_h().bg(crate::theme::accent()))
}

/// The history's Play: the title, at the episode the core picked (none: the title's own pick).
pub(super) fn history_play(app: &mut App, effect: Effect) {
    if let Effect::Play { show, episode } = effect {
        play_saved(app, show, episode.unwrap_or_default());
    }
}

/// One of this profile's settings changed: at once here, saved behind, a "Saved" tick at the top.
fn save(app: &mut App, group: &str, key: &str, v: Value) {
    crate::profile_settings::set(group, key, v);
    app.pages.settings.send(settings::Event::Changed);
    app.kit.dirty = true;
}

/// A setting's choices, as the core names them, the one shown picked; a pick is saved as `value` makes it.
fn pick(app: &App, key: &str, c: &settings::Choice, current: &str, value: fn(&str) -> Value) -> El {
    let options: Vec<(String, &str)> = c.options.iter().map(|o| (o.value.clone(), o.label.as_str())).collect();
    let (group, name) = (c.group.clone(), c.key.clone());
    choice(&c.name, ds::segmented(app, key, &options, current, move |a, v: String| save(a, &group, &name, value(&v))))
}

/// Downloads' and the history's head, as a section's (no Back: the rail opens them): the page's name, search at the
/// right (searching right here, the list narrowing as it's typed; `typed` told of each change) and whatever the page
/// adds (settings).
#[allow(clippy::too_many_arguments)]
fn finding_bar(p: &Pages, app: &App, page: &'static str, name: &str, hint: &str, solid: bool, extra: Option<El>, typed: Option<Cb>) -> El {
    let width = view_w(app);
    let gutter = super::shell::gutter(width);
    let hint = hint.to_string();
    let field = move |_: &Pages, app: &App, w_: f32| Pages::local_field(app, &super::find_field(page), &hint, w_, typed.clone());
    ds::head(app, &format!("{page}-head"), gutter, None, None, solid, Some(p.finding_head(app, page, name, p.finding == Some(page), width - gutter * 2., field, extra)))
}

/// A download's picture at its card's size.
fn sharp(url: &str) -> String {
    net::card_img(url, zcore::pictures::LANDSCAPE)
}

/// How far along, as a thin ring (w20 track, ink 2 px from the top), with a control inside it.
pub(super) fn ring(key: &str, share: f32, glyph: &'static str) -> El {
    // (as big as the buttons beside it: retry, delete)
    let side = Size::M.h();
    let arc = (share > 0.).then(|| el().cover().decorative().progress_arc(t::K_DOWNLOADS_RING_STROKE, t::C_INK, share));
    col().key(key).size(side, side).center().radius(side / 2.).border(t::K_DOWNLOADS_RING_STROKE, w(t::W20))
        .maybe(arc)
        .kid(icon(glyph, t::K_DOWNLOADS_RING_ICON, t::C_WHITE))
}

/// The one control every download has (`zcore::downloads::tap`), on title pages, a series' chapters and Downloads:
/// the arrow to download (`add`); going or waiting, its ring with pause; paused, the ring with play; failed, retry;
/// down, the tick (`open`).
pub(super) fn download_control(app: &App, key: &str, download: &str, progress: Option<(downloads::State, f32)>, add: Rc<dyn Fn(&mut App)>, open: Rc<dyn Fn(&mut App)>) -> El {
    use zcore::downloads::Act;
    let (state, share) = progress.map_or((None, 0.), |(s, f)| (Some(s), f));
    let act = zcore::downloads::tap(state);
    let target = download.to_string();
    let go = move |a: &mut App| match act {
        Act::Add => add(a),
        Act::Open => open(a),
        _ => downloads::act(&target, act),
    };
    match act {
        // (a bare sign, as big as the page head's: no round behind it, as the phone's)
        Act::Add | Act::Open => ds::head_button(app, key, act_glyph(act), go),
        // (try again on a soft round, as on the phone)
        Act::Retry => ds::icon_button(app, key, Kind::Secondary, Size::M, act_glyph(act), go),
        Act::Resume | Act::Pause => tap(ring(key, share, act_glyph(act)), 0.9, go),
    }
}

/// A download control's Lucide icon for what it does.
pub(super) fn act_glyph(act: zcore::downloads::Act) -> &'static str {
    use zcore::downloads::Act;
    match act {
        Act::Add => "download",
        Act::Pause => "pause",
        Act::Resume => "play",
        Act::Retry => "rotate_cw",
        Act::Open => "done",
    }
}

fn history_row(r: &HistoryRow, width: f32) -> El {
    let url = net::img(r.picture.as_deref().unwrap_or_default());
    let still = picture(el().size(t::S_STILL_W, t::S_STILL_H).no_shrink(), &url, t::R_STILL).maybe(r.progress.map(progress_bar));
    let tapped = r.key.clone();
    tap(
        row().key(&format!("h-{}", r.key)).w(width).pad_xy(0., t::SPACE_10).items_center()
            .kid(still)
            .kid(col().grow().margin(t::SPACE_12, 0., 0., 0.)
                .kid(text(&r.title, ts(t::T_ITEM_TITLE, Font::Bold, t::C_WHITE, 2)).fill_w())
                .kid(text(&r.line, ts(t::T_NOTE, Font::Sans, w(t::W50), 1)).fill_w().margin(0., t::K_CONTROL_LINE_GAP, 0., 0.))),
        t::PRESS_ROW,
        move |a| a.pages.history.send(zcore::history::Event::Tapped { key: tapped.clone() }),
    )
}

/// Play from a core screen: the title, and the episode it named (none: the player picks up where it was left).
pub(super) fn play_saved(app: &mut App, title_id: String, key: String) {
    let session = app.pages.session;
    spawn(async move {
        let title = data::title(&title_id).await;
        post(move |a| {
            if a.pages.session != session { return; }
            match title {
                Ok(title) => {
                    // (no episode named: the core's player picks up where the show was left)
                    let ep = title.episodes.iter().find(|e| e.id == key).cloned();
                    crate::remote::play(a, title, ep);
                }
                Err(e) => a.pages.note = e.message,
            }
        });
    });
}

pub(super) fn offline_play(app: &mut App, d: &Download) {
    fn episode(d: &Download) -> Option<Episode> {
        Some(Episode { id: d.episode_id.clone()?, n: d.episode, season: d.season, thumb: d.picture.clone(), ..Default::default() })
    }
    // (every episode of it that's down, in order, for Next episode with no connection)
    let episodes: Vec<_> = downloads::items().iter().filter(|i| i.state == downloads::State::Done && i.title_id == d.title_id).filter_map(episode).collect();
    let title = Arc::new(Title { id: d.title_id.clone(), title: d.title.clone(), imdb: d.imdb.clone(), cover: d.picture.clone(), episodes, ..Default::default() });
    player::open(app, title, episode(d));
}

pub(super) fn choose_picture(app: &App) -> Option<String> {
    app.window.pick_file("Pictures", "*.jpg;*.jpeg;*.png;*.webp;*.gif;*.bmp")
}

/// Back from the bootstrapper's small window to the app's, maximised (asked of the OS, so the resize is seen).
fn maximise_again(app: &App) {
    app.window.command(crate::platform::WindowCommand::Maximise);
}

pub(super) fn install(app: &mut App, url: String) {
    if app.pages.updating { return; }
    app.pages.updating = true;
    spawn(async move {
        use tokio::io::AsyncWriteExt;
        let path = net::root().join(crate::platform::install::UPDATE_FILE);
        let partial = path.with_extension("part");
        let result = async {
            let mut response = net::plain().get(url).send().await.ok()?.error_for_status().ok()?;
            let whole = response.content_length().unwrap_or(0);
            let (mut done, mut shown) = (0u64, 0u64);
            let mut out = tokio::fs::File::create(&partial).await.ok()?;
            while let Some(bytes) = response.chunk().await.ok()? {
                out.write_all(&bytes).await.ok()?;
                done += bytes.len() as u64;
                if whole > 0 && (done - shown >= 512 * 1024 || done == whole) {
                    shown = done;
                    post(move |a| a.updater.send(zcore::update::Event::Progress { done, total: whole }));
                }
            }
            out.flush().await.ok()?;
            drop(out);
            std::fs::rename(&partial, &path).ok()?;
            let ready = path.clone();
            let made = tokio::task::spawn_blocking(move || {
                crate::platform::install::prepare_update(&ready, |share| post(move |a| {
                    a.pages.finishing = Some(share);
                    a.kit.dirty = true;
                }))
            })
            .await
            .ok()?;
            (made && crate::platform::install::start_update(&path)).then_some(())
        }
        .await;
        if result.is_some() {
            // (gone from sight at once: the installer's window, in the same place, carries on the one update; shown
            // until it closed, two windows stood there a moment)
            post(|a| {
                a.window.hide();
                a.window.close();
            });
        } else {
            let _ = std::fs::remove_file(partial);
            post(|a| {
                a.pages.updating = false;
                a.pages.finishing = None;
                a.updater.send(zcore::update::Event::Failed);
                maximise_again(a);
                // (as it opened: the splash carries on as it is, the core letting it go on; else back to the pages)
                if !std::mem::take(&mut a.pages.booting) {
                    a.pages.checking = false;
                }
            });
        }
    });
}
