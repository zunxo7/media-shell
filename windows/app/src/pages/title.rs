//! A title's page, flat: the picture as a band across the top (not the whole screen), melting into the black, its logo
//! or name, facts, how far you are and the buttons over its lower left; under it the story beside the cast and
//! genres; then the episodes as a list (each with its still, name, length and what happens), or more like it.
use super::shell::{gutter, page_head};
use zcore::title_page::{EpisodeRow, Event as TitleEvent, View as TitleView};
use super::*;

impl Pages {
    /// A title's page, as the core's has it (`zcore::title_page`).
    pub(super) fn title_view(&self, app: &App, id: &str) -> El {
        let (width, height) = (view_w(app), app.kit.height);
        let gutter = gutter(width);
        let col_w = (width - gutter * 2.).min(t::K_TITLE_COLUMN);
        let col_left = (width - col_w) / 2.;
        // (the picture a band, not the whole screen: about half the window's height)
        let band_h = band_height(app);
        let _ = height;
        let scroll_key = format!("page-title-{id}");
        let pos = app.kit.scroll_of(k(&scroll_key)).pos;
        let back_button = page_head(app, "title-head", None, false, None);
        let v = self.title.view();
        // (another title's page still showing while this one's opens: its skeleton)
        let showing = v.title.as_ref().filter(|t| t.id == id && self.title_open == id);
        if let (Some(t), None) = (&v.trouble, showing) {
            return el().cover()
                .kid(trouble("title-trouble", &t.message, t.offline, t.busy, |a| a.pages.title.send(TitleEvent::Retry), None))
                .kid(back_button);
        }
        let Some(title) = showing else {
            return el().cover().kid(title_skeleton(width, band_h, gutter)).kid(back_button);
        };
        let title = Arc::new(title.clone());
        let art_url = net::img(zcore::pictures::stage(title.cover_big.as_deref(), title.cover.as_deref(), title.poster.as_deref()).unwrap_or_default());
        let glow = art_or_grey(&art_url);
        let accent = lerp(glow, t::C_WHITE, 0.08);
        let bright = lerp(glow, t::C_WHITE, 0.45);

        // ---- the band: the picture, melting into the black at its foot, darkened from the left for the words over it
        // (the picture itself is laid across the whole window behind the page, still as the page scrolls over it,
        // and the rail shows the same picture beside it: see title_art. Here, just the room for the words over it.)
        let band = el().fill_w().h(band_h).no_shrink();

        // its logo (an English one), its name otherwise
        let words_w = (col_w * t::K_TITLE_WORDS_SHARE).min(t::K_TITLE_WORDS_MAX);
        let checked = zcore::art::worth_asking(Some(&title.source), title.textless);
        let logo = title.logo.as_deref().filter(|l| !l.is_empty())
            .filter(|l| zcore::art::logo_shown(checked, if checked { crate::images::latin_logo(&net::img(l)) } else { None }, None));
        let name: El = match logo {
            Some(l) => el().w(words_w.min(t::K_TITLE_LOGO_W)).h(t::K_TITLE_LOGO_H).image(net::img(l)).contain().backdrop(0., 250),
            None => text(&title.title, ts(t::T_TV_TITLE_NAME, Font::Bold, t::C_WHITE, 2)).w(words_w),
        };
        let mut facts_row = row().margin(0., t::SPACE_16, 0., 0.).items_center();
        if let Some(r) = &title.rating {
            facts_row = facts_row.kid(imdb(r, t::T_TITLE_FACTS.0));
            if !v.facts.is_empty() { facts_row = facts_row.kid(line("   ·   ", t::T_TITLE_FACTS, Font::Medium, w(t::W40))); }
        }
        facts_row = facts_row.kid(line(&v.facts, t::T_TITLE_FACTS, Font::Medium, w(t::W85)));
        if let Some(age) = &v.age {
            facts_row = facts_row.kid(row().margin(t::SPACE_12, 0., 0., 0.).pad_xy(t::SPACE_6, t::K_CONTROL_BADGE_PAD_Y).radius(t::R_AGE).border(t::S_HAIRLINE, w(t::W40)).kid(line(age, t::T_AGE, Font::Medium, w(t::W85))));
        }
        let mut over = col().w(words_w).items_start()
            .kid(col().w(words_w).h(t::K_TITLE_LOGO_H).justify_end().items_start().kid(name))
            .kid(facts_row);
        // how far you are (a film, or the episode you're on), in the title's own colour
        if let Some(watching) = &v.watching {
            over = over.kid(row().items_center().margin(0., t::SPACE_16, 0., 0.)
                .kid(el().w(t::K_TITLE_WATCHING_BAR).h(t::S_PROGRESS).radius(t::S_PROGRESS / 2.).bg(w(t::W25)).clip().kid(el().pct_w(watching.share).fill_h().bg(bright)))
                .kid(line(&watching.line, t::T_TITLE_FACTS, Font::Medium, w(t::W70)).margin(t::SPACE_12, 0., 0., 0.)));
        }
        let size = self.size.as_ref().filter(|(of, _)| of == id).map(|(_, bytes)| *bytes);
        let mut buttons = row().gap(t::SPACE_12).margin(0., t::SPACE_20, 0., 0.).items_center()
            .kid(if v.locked {
                tick_while_locked();
                ds::button(app, "title-play", ds::Kind::Secondary, ds::Size::L, None, &v.play, |_| {})
            } else {
                ds::button(app, "title-play", ds::Kind::Primary, ds::Size::L, Some("play"), &v.play, |a| a.pages.title.send(TitleEvent::Play))
            });
        if let Some(second) = self.second_button(app, &title, v, size) {
            buttons = buttons.kid(second);
        }
        // (a show's every episode, not the extras, in one go, as the phone's Download all)
        let show_episodes: Vec<Episode> = title.episodes.iter().filter(|e| e.season.unwrap_or(1) > 0).cloned().collect();
        if !title.film && !show_episodes.is_empty() {
            if let Some(all) = download_button(app, "title-download-all", &title, show_episodes, size) {
                buttons = buttons.kid(all);
            }
        }
        over = over.kid(buttons);
        let band = band.kid(over.at(col_left, f32::NAN, f32::NAN, t::K_TITLE_WORDS_BOTTOM));

        // ---- under it: the story, and beside it the cast and genres
        let side_w = (col_w * t::K_TITLE_SIDE_SHARE).min(t::K_TITLE_SIDE_MAX);
        let mut story = col().grow();
        if !v.story.is_empty() {
            story = story.kid(tap(
                el().key("title-story").fill_w().kid(text(&v.story, ts(t::T_STORY, Font::Sans, w(t::W85), if v.story_open { 0 } else { 4 })).fill_w()),
                1.,
                |a| a.pages.title.send(TitleEvent::Story),
            ));
        }
        if !self.note.is_empty() {
            story = story.kid(text(&self.note, ts(t::T_ERROR, Font::Sans, t::C_INK2, 2)).fill_w().margin(0., t::SPACE_12, 0., 0.));
        }
        let labelled = |name: &str, value: &str| {
            row().fill_w().items_start().margin(0., 0., 0., t::SPACE_10)
                .kid(text(format!("{name}: "), ts(t::T_ABOUT_LINE, Font::Sans, w(t::W50), 1)))
                .kid(text(value, ts(t::T_ABOUT_LINE, Font::Sans, t::C_WHITE, 3)).grow())
        };
        let mut side = col().w(side_w).no_shrink();
        if let Some(cast) = &v.cast {
            side = side.kid(labelled(&v.cast_label, cast));
        }
        if let Some(genres) = &v.genres {
            side = side.kid(labelled(&v.genres_label, genres));
        }
        let about = row().w(col_w).items_start().gap(t::SPACE_40).margin(0., t::K_TITLE_ABOUT_TOP, 0., 0.).kid(story).kid(side);

        let mut body = col().fill_w().no_shrink().items_center().kid(band).kid(about);
        if !v.episodes.is_empty() {
            body = body.kid(self.episodes(app, id, &title, v, col_w, accent, bright));
        } else if !v.related.is_empty() {
            let gap = t::S_CARD_GAP;
            let columns = ((col_w + gap) / (t::K_TITLE_RELATED_MIN + gap)).floor().max(2.) as usize;
            let card = (col_w - gap * (columns - 1) as f32) / columns as f32;
            let mut grid = col().w(col_w).gap(t::SPACE_16);
            for (r_, chunk) in v.related.chunks(columns).enumerate() {
                grid = grid.kid(row().fill_w().gap(gap).kids(chunk.iter().enumerate().map(|(i, c)| {
                    let index = (r_ * columns + i) as u32;
                    landscape_card(&format!("related-{id}-{r_}-{i}"), &c.item, card, c.tag.as_deref(), c.top10, None, None,
                        move |a| a.pages.title.send(TitleEvent::Related { index }))
                }).collect()));
            }
            body = body.kid(col().w(col_w).margin(0., t::SPACE_40, 0., 0.)
                .kid(line(&v.related_title, t::T_TV_MORE_LIKE_THIS, Font::Bold, t::C_WHITE).margin(0., 0., 0., t::SPACE_16))
                .kid(grid));
        }
        body = body.kid(el().h(t::K_TITLE_FOOT).no_shrink());

        // the standard head, as every page has it: its name and a solid ground as soon as the page scrolls
        let past = pos > 4.;
        let _ = back_button;
        // (lit in the title's own colour, from its logo, as Home is; under its picture, the rail's beside it)
        let colour = glow_colour(title.logo.as_deref(), Some(art_url.clone()).filter(|u| !u.is_empty()));
        let (now, before) = self.glow_of(&format!("title-{id}"), colour);
        let _ = (now, before);
        el().cover()
            .maybe(self.title_art(app, id, "page", 0.))
            .kid(super::shell::scroller(&scroll_key).kid(body))
            .kid(page_head(app, "title-head", past.then_some(title.title.as_str()), past, None))
    }

    /// A title's picture across the whole window (`left`: how far the window's edge is to the left of where it's
    /// placed): fading to nothing at its foot and toward the window's left edge, so the rail beside the page shows the
    /// same picture melting away under it, one picture, no edge where they meet. It stays put as the page scrolls,
    /// dimming as it goes (both copies alike, so they never part).
    pub(super) fn title_art(&self, app: &App, id: &str, key: &str, left: f32) -> Option<El> {
        let title = self.title.view().title.as_ref().filter(|t| t.id == id)?;
        let url = net::img(zcore::pictures::stage(title.cover_big.as_deref(), title.cover.as_deref(), title.poster.as_deref())?);
        // (the glow comes first, the picture after it: held until the title's colour is known, or a moment's passed)
        let glow_ready = self.glows.borrow().get(&format!("title-{id}")).is_some_and(|g| g.0 != 0);
        let waited = self.title_at.as_ref().is_none_or(|(t, at)| t != id || at.elapsed().as_millis() > 1200);
        if !glow_ready && !waited {
            return None;
        }
        let pos = app.kit.scroll_of(k(&format!("page-title-{id}"))).pos;
        let band_h = band_height(app);
        let strength = (1. - pos / band_h * 0.85).clamp(0.15, 1.);
        // (the page's width: it melts away to nothing at the page's left edge, beside the rail)
        Some(el().key(&format!("{key}-art-{id}")).at(-left, -crate::WINDOW_STRIP, f32::NAN, f32::NAN).size(view_w(app) + left, band_h + crate::WINDOW_STRIP).decorative().opacity(strength).ease(super::shell::SCROLL_EASE)
            .image(url).backdrop(0.3, 250).shade_as(4))
    }

    /// The second button: From E1 once you're into a show, or Download for a film (its size on a white badge).
    fn second_button(&self, app: &App, title: &Arc<Title>, v: &TitleView, size: Option<u64>) -> Option<El> {
        if title.film || title.episodes.is_empty() {
            return download_button(app, "title-download", title, vec![], size);
        }
        let from = v.from_start.as_ref()?;
        Some(ds::button(app, "title-first", ds::Kind::Secondary, ds::Size::L, Some("skip_back"), from, |a| a.pages.title.send(TitleEvent::FromStart)))
    }

    #[allow(clippy::too_many_arguments)]
    fn episodes(&self, app: &App, id: &str, title: &Arc<Title>, v: &TitleView, col_w: f32, accent: u32, bright: u32) -> El {
        let mut c = col().w(col_w).margin(0., t::SPACE_40, 0., 0.);
        c = c.kid(row().fill_w().items_center()
            .kid(line(&v.episodes_title, t::T_TITLE_SECTION, Font::Bold, t::C_WHITE).grow())
            .kid(tap(row().key("episode-order").h(t::K_TITLE_ORDER_PILL).pad_xy(t::SPACE_14, 0.).radius(t::K_TITLE_ORDER_PILL / 2.).bg(w(t::W09)).items_center()
                .kid(line(&v.order, t::T_BUTTON_SMALL, Font::Bold, w(t::W85)))
                .kid(icon("chevron_down", t::K_TITLE_ORDER_CHEVRON, w(t::W85)).margin(t::SPACE_6, 0., 0., 0.).turn(if v.newest { 180. } else { 0. })),
                t::PRESS_CHIP, |a| a.pages.title.send(TitleEvent::Order))));
        if !v.seasons.is_empty() {
            let tabs = v.seasons.iter().map(|s| {
                let sid = s.id.clone();
                tap(
                    row().key(&format!("season-{}", s.id)).h(t::S_CHIP).no_shrink().pad_xy(t::SPACE_16, 0.).center().radius(t::S_CHIP / 2.)
                        .bg(if s.on { accent } else { w(t::W09) })
                        .kid(line(&s.label, t::T_CHIP, Font::Bold, if s.on { t::C_WHITE } else { w(t::W70) })),
                    t::PRESS_CHIP,
                    move |a| a.pages.title.send(TitleEvent::Season { id: sid.clone() }),
                )
            }).collect();
            c = c.kid(row().key(&format!("seasons-{id}")).fill_w().h(t::S_CHIP).scroll_x().gap(t::S_CHIP_GAP).margin(0., t::SPACE_14, 0., t::SPACE_6).kids(tabs));
        }
        let mut rows = col().fill_w().margin(0., t::SPACE_10, 0., 0.);
        for (i, e) in v.episodes.iter().enumerate() {
            if i > 0 {
                rows = rows.kid(el().fill_w().h(t::S_HAIRLINE).bg(FLAT_LINE).margin(t::SPACE_12, t::SPACE_6, t::SPACE_12, t::SPACE_6));
            }
            rows = rows.kid(episode_row(app, title, e, bright, col_w));
        }
        c.kid(rows)
    }
}

/// An episode, as a list row: its number, its still (how far you are along its foot, in the title's colour), its
/// name, its date and length, and what happens in it.
fn episode_row(app: &App, title: &Arc<Title>, e: &EpisodeRow, bright: u32, width: f32) -> El {
    let key = format!("ep-{}", e.id);
    let url = net::img(e.picture.as_deref().unwrap_or_default());
    let (still_w, still_h) = (t::K_TITLE_STILL_W, t::K_TITLE_STILL_H);
    let mut still = picture(el().size(still_w, still_h).no_shrink(), &url, t::K_TITLE_STILL_RADIUS);
    if e.share > 0. {
        still = still.kid(el().at(0., f32::NAN, 0., 0.).h(t::K_TITLE_STILL_PROGRESS).decorative().bg(w(t::W25)).kid(el().pct_w(e.share).fill_h().bg(bright)));
    }
    // every row the same: the name, then its date and length on one line (as the phone has it), then what happens
    let mut words_ = col().grow().margin(t::K_TITLE_EPISODE_GAP, 0., 0., 0.)
        .kid(text(&e.name, ts(t::T_EPISODE_TITLE, Font::Bold, t::C_WHITE, 1)).fill_w());
    if !e.line.is_empty() {
        words_ = words_.kid(line(&e.line, t::T_TITLE_FACTS, Font::Sans, w(t::W50)).margin(0., t::SPACE_4, 0., 0.));
    }
    if let Some(d) = e.description.as_deref() {
        words_ = words_.kid(text(d, ts(t::T_EPISODE_STORY, Font::Sans, w(t::W60), 2)).fill_w().margin(0., t::SPACE_6, 0., 0.));
    }
    let hovered = app.kit.hovered() == k(&key);
    let id = e.id.clone();
    let ep = title.episodes.iter().find(|x| x.id == e.id);
    tap(
        row().key(&key).w(width).pad4(t::SPACE_10, t::SPACE_20, t::K_TITLE_ROW_PAD_END, t::SPACE_20).items_center().radius(t::K_TITLE_ROW_RADIUS)
            .bg(if hovered { w(t::W04) } else { 0 })
            .kid(text(&e.number, centred(ts(t::T_EPISODE_NUMBER, Font::Medium, w(t::W60), 1))).w(t::K_TITLE_EPISODE_NUMBER).no_shrink().margin(0., 0., t::SPACE_12, 0.))
            .kid(still)
            .kid(words_)
            .maybe(ep.and_then(|ep| episode_download(app, title, ep)).map(|d| d.margin(t::K_TITLE_DOWNLOAD_GAP, 0., 0., 0.))),
        t::PRESS_ROW,
        move |a| a.pages.title.send(TitleEvent::Episode { id: id.clone() }),
    )
}

/// The download mark on an episode, by the one rule every download's control has (`zcore::downloads::tap`): the arrow
/// starts it; going (or waiting), its ring with pause pauses it; paused, the ring with play carries on; failed, retry
/// tries again; done, the tick opens Downloads.
fn episode_download(app: &App, title: &Arc<Title>, ep: &Episode) -> Option<El> {
    if !downloads::offered(title, Some(ep)) {
        return None;
    }
    let (key, id) = (format!("dl-{}", ep.id), ep.id.clone());
    let progress = downloads::progress_of(&ep.id);
    let (title, ep) = (title.clone(), ep.clone());
    let add: Rc<dyn Fn(&mut App)> = Rc::new(move |a| add_downloads(a, &title, vec![Some(ep.clone())]));
    Some(super::account::download_control(app, &key, &id, progress, add, Rc::new(|a: &mut App| a.pages.push(Route::Downloads))))
}

static TICKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A title not out yet: its countdown moves each second.
fn tick_while_locked() {
    use std::sync::atomic::Ordering;
    if TICKING.swap(true, Ordering::Relaxed) {
        return;
    }
    spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        post(|a| {
            TICKING.store(false, Ordering::Relaxed);
            a.pages.title.send(TitleEvent::Tick);
        });
    });
}

/// Adds a film (`None`) or these episodes to Downloads; only a problem is worth a line (the button shows the rest).
fn add_downloads(a: &mut App, title: &Arc<Title>, episodes: Vec<Option<Episode>>) {
    let (title, session) = (title.clone(), a.pages.session);
    spawn(async move {
        let mut failed = None;
        for ep in &episodes {
            if let Err(e) = downloads::add(&title, ep.as_ref()).await {
                failed = Some(e.message);
            }
        }
        post(move |a| if a.pages.session == session { a.pages.note = failed.unwrap_or_default() });
    });
}

/// A button with about what its download takes on a white chip over its top right corner (`zcore::downloads::size_chip`).
pub(super) fn with_size_chip(key: &str, button: El, chip: Option<String>) -> El {
    el().key(key).kid(button)
        .maybe(chip.map(|c| row().at(f32::NAN, -t::K_TITLE_SIZE_PILL_RISE, -t::SPACE_4, f32::NAN).pad4(t::SPACE_8, t::K_TITLE_SIZE_PILL_PAD_Y, t::SPACE_8, t::K_TITLE_SIZE_PILL_PAD_Y).radius(t::R_PILL).bg(t::C_WHITE).decorative()
            .kid(line(c, t::T_BADGE, Font::Bold, t::C_BLACK))))
}

/// A film's Download (`episodes` empty) or a show's Download all (its episodes, not the extras), by the core's rule for
/// several at once (`zcore::downloads::batch`): about how big while it adds, then how far, Pause, Resume, Try
/// again, and Downloaded (which opens Downloads).
fn download_button(app: &App, key: &str, title: &Arc<Title>, episodes: Vec<Episode>, size: Option<u64>) -> Option<El> {
    let one = episodes.is_empty();
    if !downloads::offered(title, episodes.first()) {
        return None;
    }
    let keys: Vec<String> = if one { vec![title.id.clone()] } else { episodes.iter().map(|e| e.id.clone()).collect() };
    let items: Vec<(Option<downloads::State>, f32)> = keys.iter().map(|k| downloads::progress_of(k).map_or((None, 0.), |(s, f)| (Some(s), f))).collect();
    let (act, words_) = zcore::downloads::batch(&items, one);
    let states: Vec<Option<downloads::State>> = items.iter().map(|i| i.0).collect();
    let targets = zcore::downloads::batch_targets(&states, act);
    let targets_n = targets.len();
    let glyph = super::account::act_glyph(act);
    let title_source = title.source.clone();
    let title = title.clone();
    let button = ds::button(app, key, ds::Kind::Secondary, ds::Size::L, Some(glyph), words_, move |a| match act {
        zcore::downloads::Act::Open => a.pages.push(Route::Downloads),
        zcore::downloads::Act::Add => add_downloads(a, &title, if one { vec![None] } else { targets.iter().map(|&i| Some(episodes[i].clone())).collect() }),
        _ => targets.iter().for_each(|&i| downloads::act(&keys[i], act)),
    });
    Some(with_size_chip(&format!("{key}-wrap"), button, zcore::downloads::size_chip(act, size, targets_n, states.len(), one && title_source == "cinema")))
}

/// How tall a title's picture is: about half the window (the same for the page and the rail beside it).
pub(super) fn band_height(app: &App) -> f32 {
    (app.kit.height * t::K_TITLE_BAND_SHARE).clamp(t::K_TITLE_BAND_MIN, t::K_TITLE_BAND_MAX).min(view_w(app) * t::K_TITLE_BAND_WIDTH_SHARE)
}

/// While a title loads: the page as it will be: the picture's band fading into the page, the name, facts and buttons
/// over its lower left, the story with the details beside it, then the episodes.
fn title_skeleton(width: f32, band_h: f32, gutter: f32) -> El {
    let col_w = (width - gutter * 2.).min(t::K_TITLE_COLUMN);
    let col_left = (width - col_w) / 2.;
    let words_w = (col_w * t::K_TITLE_WORDS_SHARE).min(t::K_TITLE_WORDS_MAX);
    let side_w = (col_w * t::K_TITLE_SIDE_SHARE).min(t::K_TITLE_SIDE_MAX);
    // the band: a soft light fading out at its foot and to the left, as the picture will
    let band = el().fill_w().h(band_h).no_shrink()
        // (no lit band of its own: its edge stood out against the rail for a moment; the page's own ground shows)
        .kid(col().at(col_left, f32::NAN, f32::NAN, t::K_TITLE_WORDS_BOTTOM).w(words_w).items_start()
            .kid(skeleton(el().w(words_w.min(t::K_TITLE_LOGO_SKELETON_W)).h(t::K_TITLE_LOGO_SKELETON_H).margin(0., t::SPACE_24, 0., 0.), t::R_STILL))
            .kid(skeleton(el().w(t::K_TITLE_FACTS_SKELETON_W).h(t::K_TITLE_FACTS_SKELETON_H).margin(0., t::SPACE_16, 0., 0.), t::K_TITLE_FACTS_SKELETON_H / 2.))
            .kid(row().gap(t::SPACE_12).margin(0., t::SPACE_20, 0., 0.)
                .kid(skeleton(el().w(t::K_TITLE_PLAY_SKELETON_W).h(t::K_TITLE_BUTTON_SKELETON_H), t::K_TITLE_BUTTON_SKELETON_H / 2.))
                .kid(skeleton(el().w(t::K_TITLE_MORE_SKELETON_W).h(t::K_TITLE_BUTTON_SKELETON_H), t::K_TITLE_BUTTON_SKELETON_H / 2.))));
    let lines = |widths: &[f32]| col().fill_w().gap(t::SPACE_12).kids(widths.iter().map(|p| skeleton(el().pct_w(*p).h(t::K_TITLE_ABOUT_SKELETON_H), t::K_TITLE_ABOUT_SKELETON_H / 2.)).collect());
    let about = row().w(col_w).items_start().gap(t::SPACE_40).margin(0., t::K_TITLE_ABOUT_TOP, 0., 0.)
        .kid(col().grow().kid(lines(&[0.96, 0.9, 0.55])))
        .kid(col().w(side_w).no_shrink().kid(lines(&[0.8, 0.6])));
    let mut episodes = col().w(col_w).margin(0., t::SPACE_40, 0., 0.)
        .kid(row().fill_w().items_center().kid(skeleton(el().w(t::K_TITLE_HEADING_SKELETON_W).h(t::K_TITLE_HEADING_SKELETON_H), t::R_SMALL)).kid(el().grow()).kid(skeleton(el().w(t::K_TITLE_ORDER_SKELETON_W).h(t::K_TITLE_ORDER_PILL), t::K_TITLE_ORDER_PILL / 2.)))
        .kid(row().gap(t::S_CHIP_GAP).margin(0., t::SPACE_14, 0., t::SPACE_16).kid(skeleton(el().w(t::K_TITLE_SEASON_SKELETON_SHORT).h(t::S_CHIP), t::S_CHIP / 2.)).kid(skeleton(el().w(t::K_TITLE_SEASON_SKELETON_LONG).h(t::S_CHIP), t::S_CHIP / 2.)));
    for _ in 0..4 {
        episodes = episodes.kid(row().fill_w().items_center().gap(t::K_TITLE_EPISODE_GAP).margin(0., 0., 0., t::K_TITLE_EPISODE_SKELETON_BELOW)
            .kid(el().w(t::K_TITLE_NUMBER_SKELETON_W).no_shrink())
            .kid(skeleton(el().size(t::K_TITLE_STILL_W, t::K_TITLE_STILL_H).no_shrink(), t::K_TITLE_STILL_RADIUS))
            .kid(col().grow().gap(t::SPACE_10).kid(skeleton(el().w(t::K_TITLE_EPISODE_NAME_SKELETON_W).h(t::K_TITLE_EPISODE_NAME_SKELETON_H), t::K_TITLE_EPISODE_NAME_SKELETON_H / 2.)).kid(skeleton(el().w(t::K_TITLE_EPISODE_LINE_SKELETON_W).h(t::K_TITLE_EPISODE_LINE_SKELETON_H), t::K_TITLE_EPISODE_LINE_SKELETON_H / 2.))));
    }
    col().fill_w().items_center().kid(band).kid(about).kid(episodes)
}

