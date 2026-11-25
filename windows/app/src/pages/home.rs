//! Home: a stage at the top (a rounded panel with the featured title's art bleeding in from the right, its words on
//! the left, the next ones as small pictures in its corner), the page lit in its colour; then the rows.
use super::shell::gutter;
use super::*;

/// Run with the index of the card tapped in a grid.
type CardTap = Rc<dyn Fn(&mut App, usize)>;

/// Where each of Home's rows starts (its title) and how tall it is, as home_view lays them out: (title, ranked,
/// carrying on) for each.
fn row_spans(app: &App, rows: &[(String, bool, bool)], with_hero: bool) -> Vec<(String, f32, f32)> {
    let width = view_w(app);
    let gutter = gutter(width);
    let hero_h = hero_height(width - gutter * 2., app.kit.height);
    let card = row_card_width(width, gutter);
    let mut y = if with_hero { t::SPACE_20 + hero_h } else { TOP };
    rows.iter().map(|(title, ranked, continuing)| {
        let art = if *ranked { card * t::K_HOME_ROW_RANKED_RATIO } else { card * t::K_CARD_WIDE_RATIO + if *continuing { t::K_HOME_ROW_PROGRESS_ROOM } else { 0. } };
        let tall = ROW_HEAD + art + t::SPACE_16;
        let span = (title.clone(), y, tall);
        y += tall;
        span
    }).collect()
}

impl Pages {
    /// Home's rows changed (a fresher list, Continue watching worked out): the row on screen stays where it is, the
    /// page moved by however much changed above it, so nothing jumps under the pointer.
    pub(super) fn set_home_rows(app: &mut App, rows: Vec<(String, bool, bool)>, with_hero: bool) {
        let key = k("page-home");
        let pos = app.kit.scroll_of(key).pos;
        let shift = if pos > 0. && !app.pages.rows_seen.is_empty() {
            let old = row_spans(app, &app.pages.rows_seen, with_hero);
            let new = row_spans(app, &rows, with_hero);
            // (the first row still on screen that's in both lists)
            old.iter().filter(|(_, y, tall)| y + tall > pos).find_map(|(title, y, _)| new.iter().find(|(t, _, _)| t == title).map(|(_, ny, _)| ny - y))
        } else {
            None
        };
        app.pages.rows_seen = rows;
        if let Some(by) = shift.filter(|d| d.abs() > 0.5) {
            app.kit.shift_scroll(key, by);
        }
    }
}

fn hero_height(width: f32, height: f32) -> f32 {
    (height * t::K_HERO_HEIGHT_SHARE).clamp(t::K_HERO_HEIGHT_MIN, t::K_HERO_HEIGHT_MAX).min(width * t::K_HERO_WIDTH_SHARE)
}

impl Pages {
    /// Home, as the core's Home has it (`zcore::home_page`): the stage, then the rows.
    pub(super) fn home_view(&self, app: &App) -> El {
        let (width, height) = (view_w(app), app.kit.height);
        let gutter = gutter(width);
        let scroll_key = "page-home";
        let pos = app.kit.scroll_of(k(scroll_key)).pos;
        let stage_w = width - gutter * 2.;
        let hero_h = hero_height(stage_w, height);
        let v = self.home.view();
        let mut body = col().fill_w().no_shrink();
        if !v.hero.is_empty() {
            body = body.kid(self.hero(app, v, stage_w, hero_h).margin(gutter, t::SPACE_20, 0., 0.));
        }
        if !v.rows.is_empty() {
            // (only rows near the screen are built; the rest keep their place)
            let mut y = if v.hero.is_empty() { TOP } else { t::SPACE_20 + hero_h };
            // (each row known by its title, never its place: a row added above, Continue watching arriving, must not
            // make every row under it a new one, its pictures fading in again)
            let mut seen: HashMap<&str, usize> = HashMap::new();
            for (at, r) in v.rows.iter().enumerate() {
                let index = { let n = seen.entry(r.title.as_str()).or_insert(0); *n += 1; *n };
                let card = row_card_width(width, gutter);
                let art = if r.ranked { card * t::K_HOME_ROW_RANKED_RATIO } else { card * t::K_CARD_WIDE_RATIO + if r.continuing { t::K_HOME_ROW_PROGRESS_ROOM } else { 0. } };
                let tall = ROW_HEAD + art + t::SPACE_16;
                let scope = if index == 1 { format!("row-{}", r.title) } else { format!("row-{}-{index}", r.title) };
                let near = y + tall >= pos - height && y <= pos + height * 2.;
                body = body.kid(if near { self.card_row(app, &scope, at, r, gutter) } else { el().key(&scope) }.h(tall).no_shrink());
                y += tall;
            }
        } else if v.loading {
            body = body.kid(home_skeleton(width, hero_h, gutter, v.hero.is_empty(), height));
        }
        body = body.kid(el().h(t::SPACE_48).no_shrink());
        // (the page lit in the featured title's colour, from its logo, easing off as it scrolls away, never quite gone)
        let mut page = el().cover()
            .kid(super::shell::scroller(scroll_key).kid(body));
        if let Some(t) = &v.trouble {
            let retry = |a: &mut App| a.pages.home.send(zcore::home_page::Event::Retry);
            page = page.kid(el().cover().bg(PAGE).kid(trouble("home-trouble", &t.message, t.offline, t.busy, retry, Some(open_downloads()))));
        }
        page
    }

    /// The stage: the featured title, a new one fading in over the last; the others as small pictures in its corner
    /// (the one showing ringed, a line filling across it until the next), any of them a click away.
    fn hero(&self, app: &App, v: &zcore::home_page::View, stage_w: f32, hero_h: f32) -> El {
        let slides = &v.hero;
        let current = (v.hero_at as usize).min(slides.len() - 1);
        // (square underneath, every layer flush; the rounded corners made once, by one mask over the top: each layer
        // clipped to the curve on its own left a faint seam where their soft edges met, "conflation")
        // (one rounded picture with its shading baked in, nothing clipped over it: one clean edge on any background)
        let mut stage = el().key("hero").w(stage_w).h(hero_h).no_shrink();
        if self.hero_seen.get() as usize != current {
            self.hero_from.set(Some((self.hero_seen.get() as usize, Instant::now())));
            self.hero_seen.set(current as u32);
        }
        if let Some((from, at)) = self.hero_from.get() {
            if at.elapsed().as_millis() < HERO_HOLD_MS && from < slides.len() && from != current {
                stage = stage.kid(hero_page(app, v, &slides[from], stage_w, hero_h, &format!("hero-{from}")).decorative());
            }
        }
        let fading = self.hero_from.get().is_some_and(|(from, _)| from != current);
        stage = stage.kid(hero_page(app, v, &slides[current], stage_w, hero_h, &format!("hero-{current}")).anim(if fading { Anim::Fade(500) } else { Anim::None }));
        if slides.len() > 1 {
            let tw = (stage_w * t::K_HERO_THUMB_SHARE).clamp(t::K_HERO_THUMB_MIN, t::K_HERO_THUMB_MAX);
            let th = tw * t::K_CARD_WIDE_RATIO;
            let words_room = t::K_HERO_WORDS_START + (stage_w * t::K_HERO_WORDS_SHARE).clamp(t::K_HERO_WORDS_MIN, t::K_HERO_WORDS_MAX) + t::SPACE_40;
            let fit = (((stage_w - words_room - t::K_HERO_STRIP_INSET + t::SPACE_10) / (tw + t::SPACE_10)).floor().max(0.) as usize).min(slides.len());
            let first = zcore::home_page::strip_start(current, fit);
            let mut strip = row().key("hero-strip").at(f32::NAN, f32::NAN, t::K_HERO_STRIP_INSET, t::K_HERO_STRIP_INSET).gap(t::SPACE_10).items_end();
            for (i, s) in slides.iter().enumerate().skip(first).take(if fit >= 2 { fit } else { 0 }) {
                let on = i == current;
                let key = format!("hero-pick-{i}");
                let hot = app.kit.hover_within(k(&key));
                let cover = zcore::pictures::landscape(s.cover.as_deref(), s.cover_big.as_deref(), s.poster.as_deref()).unwrap_or_default();
                let url = if cover.is_empty() { String::new() } else { net::card_img(cover, zcore::pictures::THUMB) };
                let mut thumb = picture(el().size(tw, th).no_shrink(), &url, t::K_CARD_LANDSCAPE_RADIUS).bg(t::C_CARD)
                    .kid(el().cover().decorative().bg(black(0.35)).opacity(if on || hot { 0. } else { 1. }).ease(220));
                if on {
                    // (how long until the next: a small bar inside it, on its own dark track, clear of the ring)
                    thumb = thumb.kid(el().at(t::SPACE_8, f32::NAN, t::SPACE_8, t::K_HERO_BAR_BOTTOM).h(t::K_HERO_BAR).radius(t::K_HERO_BAR / 2.).clip().bg(black(0.55)).decorative()
                        .kid(el().key(&format!("hero-timer-{i}")).fill().bg(t::C_WHITE).anim(Anim::FillX(0., i64::from(v.stay_ms)))));
                }
                let index = i as u32;
                strip = strip.kid(tap(thumb.key(&key).grow_hover(1.06), 1., move |a| a.pages.home.send(zcore::home_page::Event::Hero { index })));
            }
            stage = stage.kid(strip);
        }
        stage
    }

    /// A row (`at`: its place among the rows): its name, then its cards running off the edge; arrows at the ends
    /// while the pointer is over it.
    fn card_row(&self, app: &App, scope: &str, at: usize, r: &zcore::home_page::RowView, gutter: f32) -> El {
        let ranked = r.ranked;
        let width = row_card_width(view_w(app), gutter);
        let track = k(&format!("{scope}-track"));
        // (each card known by its title, not its place in the row: a fresher list moving things along rebuilds
        // nothing that's still there; the Top 10's places are part of what they show, so those keep theirs)
        let mut taken: HashSet<String> = HashSet::new();
        let row_at = at as u32;
        let cards = r.cards.iter().enumerate().map(|(i, c)| {
            let item = &c.item;
            let mut key = if ranked { format!("{scope}-{i}-{}", item.id) } else { format!("{scope}-{}{}", item.id, item.resume.as_deref().unwrap_or_default()) };
            if !taken.insert(key.clone()) { key = format!("{key}-{i}"); }
            // (under the pointer it grows and its details fade in: the kit plays that itself, nothing built again)
            let card = i as u32;
            let open = move |a: &mut App| a.pages.home.send(zcore::home_page::Event::Tapped { row: row_at, card });
            if ranked {
                return rank_cell(&key, item, i + 1, width, open);
            }
            // (Continue watching plays: its ⓘ, or a right-click, opens the title's page instead)
            let info: Option<crate::kit::Cb> = r.continuing.then(|| {
                std::rc::Rc::new(move |a: &mut App| a.pages.home.send(zcore::home_page::Event::Info { row: row_at, card })) as crate::kit::Cb
            });
            landscape_card(&key, item, width, c.tag.as_deref(), c.top10, c.progress, info, open)
        }).collect();
        // (room above and below for a card growing under the pointer)
        let rail = row().id(track).fill_w().scroll_x().items_end().gap(t::S_CARD_GAP).pad4(gutter, t::SPACE_8, gutter, t::SPACE_8).kids(cards);
        let mut wrap = el().key(&format!("{scope}-wrap")).fill_w().hover().kid(rail);
        let state = app.kit.scroll_of(track);
        if app.kit.hover_within(k(&format!("{scope}-wrap"))) {
            if state.pos > 4. {
                wrap = wrap.kid(row_arrow(scope, track, false));
            }
            if state.max - state.pos > 4. {
                wrap = wrap.kid(row_arrow(scope, track, true));
            }
        }
        col().key(scope).fill_w()
            .kid(line(&r.title, t::T_HOME_ROW_TITLE, Font::Bold, t::C_INK).margin(gutter, t::K_HOME_ROW_TOP, 0., t::SPACE_6))
            .kid(wrap)
    }

    /// A section, as the core's has it (`zcore::browse::Section`): its chips, then its titles; searching right here,
    /// kept to the section.
    pub(super) fn browse_view(&self, app: &App, section: &'static str) -> El {
        let (width, height) = (view_w(app), app.kit.height);
        let gutter = gutter(width);
        let scroll_key = format!("page-{section}");
        let pos = app.kit.scroll_of(k(&scroll_key)).pos;
        let screen = self.section(section);
        let v = screen.view();
        // (search right here, kept to this section: the name slides away as Back comes in, the search button gives way
        // to the field sliding in from it, the titles cross-fade into the results; Back plays it the other way)
        let finding = self.finding == Some(section);
        let field_key = super::find_field(section);
        let found_pos = app.kit.scroll_of(k(&format!("{field_key}-page"))).pos;
        let field = move |p: &Pages, app: &App, w_: f32| {
            let s = p.section(section).view();
            p.search_field(app, &super::find_field(section), &s.search.hint, w_, s.search.busy)
        };
        // (until two letters are typed, the section's own titles stay, its chips folding away; then the results)
        let typed = finding && v.search.typed;
        let head = ds::head(app, &format!("{section}-head"), gutter, None, None, if typed { found_pos > 4. } else { pos > 4. },
            Some(self.finding_head(app, section, &v.title, finding, width - gutter * 2., field, None)));
        let failed = v.trouble.is_some();
        let chips = v.chips.iter().enumerate().map(|(i, chip)| {
            let index = i as u32;
            ds::chip(app, &format!("{section}-chip-{i}"), &chip.label, chip.on, move |a| {
                a.kit.reset_scroll(k(&format!("page-{section}")));
                a.pages.section(section).send(zcore::browse::SectionEvent::Chip { index });
            })
        }).collect();
        let mut body = col().fill_w().no_shrink().pad4(0., ds::HEAD, 0., 0.);
        let mut chips_over = None;
        if !failed {
            // (the chips over the top of the list, fading as searching starts; their room closes with the grid sliding up)
            let mut row_ = chip_row(&format!("{section}-chips"), chips, gutter).at(0., ds::HEAD, 0., f32::NAN).opacity(if finding { 0. } else { 1. }).ease(SWAP_MS);
            if finding { row_ = row_.decorative(); }
            chips_over = Some(row_);
            body = body.kid(el().key(&format!("{section}-chip-room")).h(if finding { 0. } else { CHIP_ROOM }).no_shrink());
        } else {
            body = body.kid(el().h(t::SPACE_14));
        }
        if let Some(note) = &v.building {
            body = body.kid(text(note, ts(t::T_HOME_NOTE, Font::Sans, t::C_MUTED, 2)).margin(gutter, 0., gutter, t::SPACE_14));
        }
        let grid_top = ds::HEAD + if failed { t::SPACE_14 } else if finding { 0. } else { CHIP_ROOM };
        if v.loading {
            body = body.kid(grid_skeleton(width - gutter * 2., width, 6).margin(gutter, 0., gutter, 0.));
        } else {
            let tap: CardTap = Rc::new(move |a: &mut App, i: usize| a.pages.section(section).send(zcore::browse::SectionEvent::Tapped { index: i as u32 }));
            body = body.kid(poster_grid(&format!("grid-{section}"), &v.cards, width - gutter * 2., width, Some((height, pos - grid_top)), tap).margin(gutter, 0., gutter, 0.).slide(SWAP_MS));
        }
        body = body.kid(el().h(t::SPACE_40).no_shrink());
        let mut titles = el().key(&format!("{section}-titles")).cover().opacity(if typed { 0. } else { 1. }).ease(SWAP_MS)
            .kid(super::shell::scroller(&scroll_key).kid(body.maybe(chips_over)));
        if let Some(t) = &v.trouble {
            let retry = move |a: &mut App| a.pages.section(section).send(zcore::browse::SectionEvent::Retry);
            titles = titles.kid(el().cover().pad4(0., ds::HEAD, 0., 0.).kid(trouble(&format!("{section}-trouble"), &t.message, t.offline, t.busy, retry, Some(open_downloads()))));
        }
        if typed { titles = titles.decorative(); }
        let found_tap: CardTap = Rc::new(move |a: &mut App, i: usize| a.pages.section(section).send(zcore::browse::SectionEvent::Found { index: i as u32 }));
        let mut found = el().key(&format!("{section}-found")).cover().opacity(if typed { 1. } else { 0. }).ease(SWAP_MS)
            .kid(super::shell::scroller(&format!("{field_key}-page"))
                .kid(search_results(app, &format!("found-{section}"), &v.search, found_pos, ds::HEAD, found_tap)));
        if !typed { found = found.decorative(); }
        el().cover().kid(titles).kid(found).kid(head)
    }

    /// A page's head that searches right on it (a section, Downloads, Watch history): its name with search at the right
    /// (and `extra` beside it: Downloads' settings); searching, Back where the name was and the field filling the rest
    /// (`field` makes it, as wide as it's given). Every part stays, faded and slid, so going either way plays smoothly
    /// and never jumps. `page`: what `finding` names while it searches.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finding_head(&self, app: &App, page: &'static str, name: &str, finding: bool, row_w: f32,
                               field: impl Fn(&Pages, &App, f32) -> El, extra: Option<El>) -> El {
        let on = |v: bool| if v { 1. } else { 0. };
        let extra_w = if extra.is_some() { t::K_CONTROL_BUTTON_M + t::SPACE_8 } else { 0. };
        let title = row().key(&format!("{page}-name")).at(if finding { -t::SPACE_24 } else { 0. }, 0., f32::NAN, 0.).items_center()
            .slide(SWAP_MS).opacity(on(!finding)).ease(SWAP_MS).decorative()
            .kid(text(name, ts(t::T_PAGE_NAME, Font::Bold, t::C_WHITE, 1)));
        let mut back = row().key(&format!("{page}-back-lane")).at(if finding { 0. } else { -t::SPACE_24 }, 0., f32::NAN, 0.).items_center()
            .slide(SWAP_MS).opacity(on(finding)).ease(SWAP_MS)
            .kid(ds::back_button(app, &format!("{page}-back"), |a| a.pages.close_find()));
        if !finding { back = back.decorative(); }
        let mut button = row().key(&format!("{page}-search-lane")).at(f32::NAN, 0., extra_w, 0.).items_center()
            .opacity(on(!finding)).ease(SWAP_MS)
            .kid(ds::head_button(app, &format!("{page}-search"), "search", move |a| a.pages.open_find(page, &mut a.kit)));
        if finding { button = button.decorative(); }
        let field_w = row_w - ds::BACK - t::SPACE_16 - extra_w;
        let mut lane = row().key(&format!("{page}-field-lane")).at(if finding { ds::BACK + t::SPACE_16 } else { ds::BACK + t::SPACE_56 }, 0., f32::NAN, 0.).items_center()
            .slide(SWAP_MS).opacity(on(finding)).ease(SWAP_MS)
            .kid(field(self, app, field_w));
        if !finding { lane = lane.decorative(); }
        let mut head = el().key(&format!("{page}-head-row")).w(row_w).h(t::K_CONTROL_BUTTON_M).kid(title).kid(back).kid(button).kid(lane);
        if let Some(extra) = extra {
            head = head.kid(row().key(&format!("{page}-extra-lane")).at(f32::NAN, 0., 0., 0.).items_center().kid(extra));
        }
        head
    }

    /// A search field over a list that's already here (Downloads, Watch history): typed in, the list narrows; ✕ clears it.
    /// `typed`: told of each change.
    pub(super) fn local_field(app: &App, key: &str, hint: &str, field_w: f32, typed: Option<Cb>) -> El {
        let (input, spec) = field_line(app, Field { key, hint, change: typed.clone(), ..Default::default() }, ts(t::T_FIELD_TEXT, Font::Sans, t::C_WHITE, 1), w(t::W50), field_w - t::SPACE_16 - t::K_HOME_SEARCH_GLYPH - t::SPACE_10 - t::SPACE_14 - t::K_HOME_SEARCH_END_ROOM);
        let mut inside = row().key(&format!("{key}-inside")).fill().items_center().pad4(t::SPACE_16, 0., t::SPACE_14, 0.)
            .kid(icon("search", t::K_HOME_SEARCH_GLYPH, w(t::W70)))
            .kid(input.grow().margin(t::SPACE_10, 0., 0., 0.));
        if !app.kit.text_of(key).is_empty() {
            let clear = key.to_string();
            inside = inside.kid(ds::icon_button(app, &format!("{key}-clear"), ds::Kind::Ghost, ds::Size::S, "x", move |a| {
                a.kit.set_text(&clear, "");
                a.kit.focus = k(&clear);
                if let Some(typed) = &typed {
                    typed(a);
                }
            }));
        }
        let mut field = row().key(key).w(field_w).h(t::S_FIELD_SEARCH).radius(t::R_PILL).bg(w(t::W11)).kid(inside);
        field.field = Some(spec);
        field
    }

    /// Search through everything (the rail's): Back and the field in the head, the results fading up under it.
    pub(super) fn search_view(&self, app: &App) -> El {
        let (width, gutter) = (view_w(app), gutter(view_w(app)));
        let pos = app.kit.scroll_of(k("page-search")).pos;
        let v = self.finder.view();
        let field = self.search_field(app, "search", &v.hint, width - gutter * 2. - ds::BACK - t::SPACE_16, v.busy);
        // (the standard head, as every page has it: Back, then the field; its ground fading in as the page scrolls)
        let head = ds::head(app, "search-head", gutter, Some(Rc::new(|a: &mut App| a.pages.back())), None, pos > 4., Some(field));
        let tap: CardTap = Rc::new(|a: &mut App, i: usize| a.pages.finder.send(zcore::browse::SearchEvent::Tapped { index: i as u32 }));
        el().cover()
            .kid(super::shell::scroller("page-search").kid(search_results(app, "search", v, pos, t::SPACE_8 + t::S_FIELD_SEARCH + t::K_HOME_RESULTS_GAP, tap)))
            .kid(head)
    }

    /// The search field, `field_w` wide: the search icon, the words, a spinner while it looks (`busy`) or ✕ to clear it.
    pub(super) fn search_field(&self, app: &App, key: &str, hint: &str, field_w: f32, busy: bool) -> El {
        search_box(app, key, hint, field_w, Rc::new(Self::search), busy, !app.kit.text_of(key).is_empty())
    }
}

/// A search's results, as the core's search has them: its note ("Popular right now", "12 results") over the titles
/// as a poster grid; its skeleton while it looks; "Nothing found" when nothing is. `pos`: how far its scroller is
/// down; `top`: room left for the head; `tap`: a result pressed, by its place.
fn search_results(app: &App, key: &str, v: &zcore::browse::SearchView, pos: f32, top: f32, tap: CardTap) -> El {
    let (width, height) = (view_w(app), app.kit.height);
    let gutter = gutter(width);
    let mut results = col().key(&format!("{key}-results")).fill_w().no_shrink().pad4(gutter, top, gutter, t::SPACE_40)
        .kid(line(&v.note, t::T_ROW_TITLE, Font::Bold, t::C_WHITE).margin(0., t::SPACE_2, 0., t::SPACE_14));
    if let Some(e) = &v.empty {
        results = results.kid(empty("search", &e.title, &e.line));
    } else if v.cards.is_empty() && v.busy {
        results = results.kid(grid_skeleton(width - gutter * 2., width, 6));
    } else {
        results = results.kid(poster_grid(key, &v.cards, width - gutter * 2., width, Some((height, pos - top - t::SPACE_40)), tap));
    }
    results
}

/// A search field, `field_w` wide: the search icon, the words (`change` as they're typed), a spinner while it looks
/// (`busy`) or ✕ to clear it (`typed`).
pub(super) fn search_box(app: &App, key: &str, hint: &str, field_w: f32, change: Cb, busy: bool, typed: bool) -> El {
    let (input, spec) = field_line(app, Field { key, hint, change: Some(change.clone()), submit: Some(change.clone()), ..Default::default() }, ts(t::T_FIELD_TEXT, Font::Sans, t::C_WHITE, 1), w(t::W50), field_w - t::SPACE_16 - t::K_HOME_SEARCH_GLYPH - t::SPACE_10 - t::SPACE_14 - t::K_HOME_SEARCH_END_ROOM);
    let mut inside = row().key(&format!("{key}-inside")).fill().items_center().pad4(t::SPACE_16, 0., t::SPACE_14, 0.)
        .kid(icon("search", t::K_HOME_SEARCH_GLYPH, w(t::W70)))
        .kid(input.grow().margin(t::SPACE_10, 0., 0., 0.));
    if busy {
        inside = inside.kid(spinner(t::K_PAGE_FIELD_END, w(t::W60)));
    } else if typed {
        let clear = key.to_string();
        inside = inside.kid(ds::icon_button(app, &format!("{key}-clear"), ds::Kind::Ghost, ds::Size::S, "x", move |a| {
            a.kit.set_text(&clear, "");
            a.kit.focus = k(&clear);
            change(a);
        }));
    }
    // (the Back button's see-through white beside it: one ground for the whole head)
    let mut field = row().key(key).w(field_w).h(t::S_FIELD_SEARCH).radius(t::R_PILL).bg(w(t::W11)).kid(inside);
    field.field = Some(spec);
    field
}

/// Offline trouble's other way out: the downloads, opened fresh as the rail's are.
fn open_downloads() -> Cb {
    Rc::new(|a: &mut App| {
        a.pages.clear_for();
        a.pages.push(Route::Downloads);
    })
}

/// How long the name, Back, the field and the results take to trade places as a section's search opens or closes.
pub(super) const SWAP_MS: i64 = 260;
/// The room a page's chips take over its grid, with the gap under them.
pub(super) const CHIP_ROOM: f32 = t::S_CHIP + t::K_HOME_CHIPS_GAP;

/// A grid of poster cards: as many as fit at 160–220 wide (190 and up past 1100), gap 12; only rows near the screen
/// built. `tap`: a card pressed, by its place.
pub(super) fn poster_grid(scope: &str, cards: &[zcore::cards::Card], content: f32, window: f32, view: Option<(f32, f32)>, tap: CardTap) -> El {
    let (columns, card) = grid_columns(content, window);
    let stride = card * t::S_POSTER_RATIO + t::S_CARD_GAP;
    let lines = cards.len().div_ceil(columns);
    let make = |index: usize| {
        let start = index * columns;
        row().key(&format!("{scope}-line-{index}")).fill_w().h(stride).no_shrink().items_start().gap(t::S_CARD_GAP).kids(
            cards[start..(start + columns).min(cards.len())].iter().enumerate().map(|(o, c)| {
                let at = start + o;
                let key = format!("{scope}-{at}-{}", c.item.id);
                let tap = tap.clone();
                poster_card(&key, &c.item, card, move |a| tap(a, at))
            }).collect(),
        )
    };
    let rows = match view {
        Some((height, offset)) => virtual_rows(lines, stride, height, offset, make),
        None => (0..lines).map(make).collect(),
    };
    col().key(&format!("{scope}-grid")).fill_w().kids(rows)
}
pub(super) fn grid_columns(content: f32, window: f32) -> (usize, f32) {
    let least = if window >= t::K_HOME_GRID_WIDE_FROM { t::L_LARGE_POSTER_MIN } else if window >= t::K_HOME_GRID_MEDIUM_FROM { t::L_MEDIUM_POSTER_MIN } else { (content - t::S_CARD_GAP) / 2. };
    let columns = ((content + t::S_CARD_GAP) / (least + t::S_CARD_GAP)).floor().max(2.) as usize;
    let card = ((content - t::S_CARD_GAP * (columns - 1) as f32) / columns as f32).max(1.);
    (columns, card)
}
/// While a grid loads: its cards' shapes, as many as a few of its rows, softly moving.
/// A grid while it loads: as many rows of shapes as the window shows, and one more (never two rows and a gap).
pub(super) fn grid_skeleton(content: f32, window: f32, least: usize) -> El {
    let (columns, card) = grid_columns(content, window);
    let tall = crate::pages::window_height().max(t::K_HOME_GRID_SKELETON_LEAST);
    let rows = (tall / (card * t::S_POSTER_RATIO + t::S_CARD_GAP)).ceil() as usize + 1;
    let count = least.max(columns * rows);
    col().fill_w().gap(t::S_CARD_GAP).kids(
        (0..count.div_ceil(columns)).map(|r| {
            row().gap(t::S_CARD_GAP).kids((0..columns).filter(|c| r * columns + c < count).map(|_| skeleton(el().size(card, card * t::S_POSTER_RATIO).no_shrink(), t::R_POSTER_CARD)).collect())
        }).collect(),
    )
}

/// Home while it loads: the stage and two rows as their shapes, where they'll be.
fn home_skeleton(width: f32, hero_h: f32, gutter: f32, with_hero: bool, height: f32) -> El {
    let mut c = col().fill_w().no_shrink();
    if with_hero {
        c = c.kid(skeleton(el().w(width - gutter * 2.).h(hero_h).margin(gutter, t::SPACE_20, 0., 0.), t::K_HERO_CORNER));
    }
    let card = row_card_width(width, gutter);
    let per_row = ((width - gutter) / (card + t::S_CARD_GAP)).ceil().max(1.) as usize;
    // (as many rows as fill the window under the stage, and one more)
    let below = height - if with_hero { hero_h + t::SPACE_20 } else { 0. };
    let rows = (below.max(0.) / (card * t::K_CARD_WIDE_RATIO + t::SPACE_64)).ceil() as usize + 1;
    let name_top = t::K_HOME_ROW_TOP + (t::K_HOME_ROW_NAME - t::K_HOME_ROW_NAME_SKELETON_H) / 2.;
    for i in 0..rows.max(2) {
        // (each as tall as a real row, its name and cards where theirs will be: nothing jumps as they arrive)
        c = c.kid(col().fill_w().h(ROW_HEAD + card * t::K_CARD_WIDE_RATIO + t::SPACE_16).no_shrink()
            .kid(skeleton(el().size(t::K_HOME_ROW_NAME_SKELETON_W, t::K_HOME_ROW_NAME_SKELETON_H).margin(gutter, name_top, 0., 0.), t::R_SMALL))
            .kid(row().key(&format!("skeleton-row-{i}")).fill_w().clip().pad4(gutter, 0., 0., 0.).gap(t::S_CARD_GAP).margin(0., ROW_HEAD - name_top - t::K_HOME_ROW_NAME_SKELETON_H, 0., 0.)
                .kids((0..per_row).map(|_| skeleton(el().size(card, card * t::K_CARD_WIDE_RATIO).no_shrink(), t::R_STILL)).collect())));
    }
    c
}

/// A row's name and the room around it, above its cards.
const ROW_HEAD: f32 = t::K_HOME_ROW_TOP + t::K_HOME_ROW_NAME + t::SPACE_6 + t::SPACE_8;

/// The featured title on the stage: its art bleeding in from the right, darkening into the panel on the left where
/// its words are (what it is, its logo or name, facts, two lines of story, Play and More info), its own colour at the foot.
/// How long the slide going stays solid under the one coming: past its fade and its big picture's own (decoded as it
/// comes in, then faded over 400 ms); gone sooner, the new one faded in over nothing, a dip to dark.
pub(super) const HERO_HOLD_MS: u128 = 1100;

fn hero_page(app: &App, v: &zcore::home_page::View, item: &Item, stage_w: f32, hero_h: f32, key: &str) -> El {
    let url = hero_url(item).unwrap_or_default();
    let words_w = (stage_w * t::K_HERO_WORDS_SHARE).clamp(t::K_HERO_WORDS_MIN, t::K_HERO_WORDS_MAX);
    let title: El = match item.logo.as_deref().filter(|l| !l.is_empty()) {
        Some(logo) => el().w(words_w * t::K_HERO_LOGO_SHARE).h((hero_h * t::K_HERO_LOGO_HEIGHT_SHARE).clamp(t::K_HERO_LOGO_MIN, t::K_HERO_LOGO_MAX)).image(net::img(logo)).contain().backdrop(0., 300),
        None => text(&item.title, ts(t::T_HERO_TITLE, Font::Bold, t::C_WHITE, 2)).w(words_w),
    };
    let mut facts_row = row().items_center().margin(0., t::SPACE_16, 0., 0.);
    if let Some(r) = &item.rating {
        facts_row = facts_row.kid(imdb(r, t::T_TITLE_FACTS.0)).kid(line("   ·   ", t::T_TITLE_FACTS, Font::Medium, w(t::W40)));
    }
    let line_ = item.facts();
    facts_row = facts_row.kid(line(line_, t::T_TITLE_FACTS, Font::Medium, w(t::W85)));
    let story = item.description.as_deref().unwrap_or_default().trim();
    let buttons = row().gap(t::SPACE_12).items_center().margin(0., t::SPACE_24, 0., 0.)
        .kid(ds::button(app, &format!("{key}-play"), ds::Kind::Primary, ds::Size::L, Some("play"), &v.play, |a| a.pages.home.send(zcore::home_page::Event::HeroPlay)))
        .kid(ds::button(app, &format!("{key}-info"), ds::Kind::Secondary, ds::Size::L, Some("info"), &v.info, |a| a.pages.home.send(zcore::home_page::Event::HeroInfo)));
    // (the logo or name, its facts and the story: no chips above it)
    let mut words_ = col().w(words_w).items_start().kid(col().w(words_w).kid(title)).kid(facts_row);
    if !story.is_empty() {
        words_ = words_.kid(text(story, ts(t::T_HERO_STORY, Font::Sans, w(t::W70), 2)).w(words_w).margin(0., t::SPACE_12, 0., 0.));
    }
    words_ = words_.kid(buttons);
    el().key(key).cover()
        .kid(el().cover().decorative().radius(t::K_HERO_CORNER).image(url).shade().backdrop(0.25, 400))
        .kid(words_.at(t::K_HERO_WORDS_START, f32::NAN, f32::NAN, t::K_HERO_WORDS_BOTTOM))
}

fn row_arrow(scope: &str, track: u64, right: bool) -> El {
    let glyph = if right { "chevron_right" } else { "chevron_left" };
    let arrow = round(&format!("{scope}-{glyph}"), glyph, t::K_HOME_ROW_ARROW, t::K_HOME_ROW_ARROW_ICON, move |a| {
        let dir = if right { 1. } else { -1. };
        let at = a.kit.scroll_of(track).pos + dir * a.kit.width * 0.75;
        let _ = a.kit.glide(track, at);
    });
    col().key(&format!("{scope}-{glyph}-lane")).at(if right { f32::NAN } else { 0. }, 0., if right { 0. } else { f32::NAN }, 0.).w(t::K_HOME_ROW_ARROW_LANE).center().kid(arrow.anim(Anim::Fade(160)))
}

pub(super) fn virtual_rows(len: usize, stride: f32, viewport: f32, offset: f32, mut row_at: impl FnMut(usize) -> El) -> Vec<El> {
    let first = (((offset - viewport) / stride).floor().max(0.) as usize).min(len);
    let last = (((offset + viewport * 2.) / stride).ceil().max(first as f32) as usize).min(len);
    let mut rows = Vec::with_capacity(last - first + 2);
    if first > 0 {
        rows.push(el().key("before").h(first as f32 * stride).no_shrink());
    }
    rows.extend((first..last).map(&mut row_at));
    if last < len {
        rows.push(el().key("after").h((len - last) as f32 * stride).no_shrink());
    }
    rows
}

