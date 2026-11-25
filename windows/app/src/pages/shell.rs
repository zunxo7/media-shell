//! The frame around the pages: the splash, sign-in, the bar along the top with the sections, the profile's menu,
//! the glass sheets, the update card, and how pages come and go.
use super::*;

impl Pages {
    pub fn view(&self, app: &App) -> El {
        crate::pages::set_window_height(app.kit.height);
        let mut root = col().fill().bg(PAGE);
        if self.checking {
            // (before the window's up and maximised: just the dark ground, the splash begun once it is)
            if self.splash_from.is_none() {
                return root.bg(t::C_BG);
            }
            // (an update made ready by the app itself, as the Mac's is: the installer's last part, in this same spot)
            if let Some(share) = self.finishing {
                let said = zcore::setup::finishing(share);
                return root.kid(crate::ui::bootstrap_view(app, &said.title, &said.detail, said.share));
            }
            if let Some(update) = &app.updater.view().bootstrap {
                // (one update, one bar: the download fills it to `setup::DOWNLOADED`, the installer, in this same spot, the rest)
                return root.kid(crate::ui::bootstrap_view(app, &update.title, &update.line, Some(zcore::setup::download_bar(update.share))));
            }
            return root.kid(splash(app, self.splash_from.map_or(f32::INFINITY, |at| at.elapsed().as_secs_f32() * 1000.)));
        }
        if self.maintenance {
            return root.kid(self.maintenance_view(app));
        }
        if self.waiting {
            return root.kid(self.waiting_view(app));
        }
        if !self.signed_in {
            root = root.kid(self.login(app));
            // (the splash eases away as the sign-in comes in under it: the card rising into place, never just there)
            if self.splash_gone.is_some() {
                root = root.kid(splash(app, f32::INFINITY).key("splash-leaving").anim(Anim::FadeOut(i64::from(t::D_OPEN))).decorative());
            }
            return root;
        }
        if self.who {
            root = root.kid(self.who_view(app, false)).maybe(self.sheet_view(app));
            if self.splash_gone.is_some() {
                root = root.kid(splash(app, f32::INFINITY).key("splash-leaving").anim(Anim::FadeOut(220)).decorative());
            }
            return root;
        }
        let moving = self.moving.as_ref().map(|(_, m)| m);
        // (everything beside the rail)
        let shell = root;
        // (as the rail opens or closes the pages dim for a moment, lay themselves out beside it, and glide back in)
        // (no way in of its own: whatever was over it, the splash or Who's watching, eases away on top of it)
        // (under the window's strip along the top)
        // (from the window's very top: the pages' own content starts under the strip, but a title's picture and a
        // head's ground run on up under it, seamless)
        let mut root = el().key("stage").at(STAGE_X, 0., 0., 0.).clip().slide(300)
            .opacity(if self.rail_moving { 0.25 } else { 1. }).ease(if self.rail_moving { 90 } else { 360 });
        // The sections and every page opened over them stay in place (so going back never rebuilds a page, its pictures
        // fading in again): the top one where it belongs, each under it a quarter to the left and out of reach. A page
        // opening slides in from the right as the one under it slides a quarter left; closing is the same, reversed,
        // on the same curve and time: it slides out right as the one under it slides back.
        let n = self.stack.len();
        let back = 0.;   // (the page under one stays still: no sliding)
        let _ = view_w(app);
        // (the pages are see-through, the window's one gradient and glow behind them all: the one on top shows, those
        // under it fade away as it comes in and back as it leaves, so nothing under it shows through and no page has
        // a ground of its own to fall out of line with the rail's)
        // (a page under the top one, settled (its way out played, nothing changed since it was last built, the window
        // the same size): kept as it is on screen, not built again; built afresh as soon as anything changes)
        let kept = |key: &str| -> bool {
            let mut frozen = self.frozen.borrow_mut();
            if moving.is_some() {
                frozen.remove(key);
                return false;
            }
            let print = (self.changes, app.kit.width.to_bits(), app.kit.height.to_bits(), self.rail_open);
            if frozen.get(key) == Some(&print) && app.kit.can_retain(k(key)) {
                return true;
            }
            if frozen.len() > 32 { frozen.clear(); }
            frozen.insert(key.to_string(), print);
            false
        };
        let slot = |key: String, top: bool, body: &dyn Fn() -> El| {
            let x = if top { 0. } else { back };
            if !top && kept(&key) {
                return el().key(&key).at(x, 0., -x, 0.).retain();
            }
            let s = el().key(&key).at(x, crate::WINDOW_STRIP, -x, 0.).slide(PAGE_SLIDE).opacity(if top { 1. } else { 0. }).ease(if top { 240 } else { 180 }).kid(body());
            if top { s } else { s.decorative() }
        };
        root = root.kid(slot("sections-slot".into(), n == 0, &|| self.sections(app)));
        for (i, route) in self.stack.iter().enumerate() {
            let top = i + 1 == n;
            // (one page for another from the rail: the old one stays, as it was, under the new one fading in)
            if let (true, Some(Move::Swap(old))) = (top, moving) {
                root = root.kid(slot(format!("slot-{old:?}"), false, &|| self.page(app, old, Anim::None, "")));
            }
            let mut page = slot(format!("slot-{route:?}"), top, &|| self.page(app, route, Anim::None, ""));
            if top && matches!(moving, Some(Move::Push) | Some(Move::Swap(_))) {
                page = page.anim(Anim::Page);
            }
            root = root.kid(page);
        }
        // (the page closing: the very node it was while open, fading out on top; nothing is made afresh, so its
        // pictures don't fade in again as it leaves, and every page leaves the same way)
        if let Some(Move::Pop(left)) = moving {
            root = root.kid(slot(format!("slot-{left:?}"), false, &|| self.page(app, left, Anim::None, "")).opacity(0.).ease(180));
        }
        // (the window's gradient, and over it the glow of the page showing, Home's or a title's, once across the whole
        // window, under the rail and the pages alike: one light, never split. A new page's glow fades in as the last
        // one's fades out)
        let current = self.glow_name();
        let leaving = {
            let mut shown = self.glow_shown.borrow_mut();
            if shown.0 != current {
                if let Some(old) = shown.0.take() { shown.1 = Some((old, Instant::now())); }
                shown.0 = current.clone();
            }
            if shown.1.as_ref().is_some_and(|(_, at)| at.elapsed().as_millis() > 600) { shown.1 = None; }
            shown.1.as_ref().map(|(name, _)| name.clone())
        };
        let glow = |name: &str, on: bool| -> Option<El> {
            let (now, before) = self.glows.borrow().get(name).copied().map(crate::pages::shown_glow)?;
            let scroll = if name == "home" { "page-home".to_string() } else { format!("page-{name}") };
            let strength = zcore::pictures::glow_strength(app.kit.scroll_of(k(&scroll)).pos, app.kit.height);
            Some(el().key(&format!("glow-{name}")).cover().decorative().anim(Anim::Fade(300)).opacity(if on { 1. } else { 0. }).ease(300)
                .kid(el().key(&format!("glow-k-{name}")).cover().opacity(strength).ease(SCROLL_EASE).kid(glow_layers(&format!("glow-l-{name}"), now, before, 0.))))
        };
        let glow_out = leaving.filter(|l| Some(l) != current.as_ref()).and_then(|l| glow(&l, false));
        let glow_in = current.as_deref().and_then(|c| glow(c, true));
        let _ = moving;
        let mut root = shell.kid(page_ground("app-ground", 0.)).maybe(glow_out).maybe(glow_in).kid(root).kid(self.rail_view(app));
        root = root.maybe(self.tv_bar(app)).maybe(self.menu_view(app)).maybe(self.sheet_view(app)).maybe(self.update_toast(app));
        // (Who's watching, just left: the very same screen fading away over Home, nothing black between them)
        if let Some(at) = self.who_leaving {
            // (its page's zoom not played again in this copy fading out)
            root = root.kid(self.who_view(app, true).decorative().opacity(0.).ease(420));
            if app.session.view().who_mark && at.elapsed().as_secs_f32() * 1000. < MORPH_MS {
                root = root.kid(brand_morph(app, crate::theme::flix()));
            }
        }
        // (the splash eases away over whatever's first: fading, a touch smaller)
        if self.splash_gone.is_some() {
            root = root.kid(splash(app, f32::INFINITY).key("splash-leaving").anim(Anim::FadeOut(220)).decorative());
        }
        root
    }

    fn page(&self, app: &App, route: &Route, anim: Anim, prefix: &str) -> El {
        let body = match route {
            Route::Search => self.search_view(app),
            Route::Title(id) => self.title_view(app, id),
            Route::Account => self.account_view(app),
            Route::Downloads => self.downloads_view(app),
            Route::History => self.history_view(app),
            Route::Manhwa => self.manhwa_view(app),
            Route::Series(path) => self.series_view(app, path),
            Route::Reader(path) => self.reader_view(app, path),
            Route::YourTv => self.your_tv_view(app),
        };
        el().key(&format!("{prefix}route-{route:?}")).cover().anim(anim).kid(body)
    }

    /// Home and the sections. Switching, the new one comes in on the same fade-through as every page; the one left
    /// fades out as it was (the same node, so its pictures don't fade in again).
    fn sections(&self, app: &App) -> El {
        let mut layer = el().key("sections").cover();
        let name = tab_key(self.tab);
        let mut anim = Anim::None;
        if let Some((at, Move::Tab(from))) = &self.moving {
            if at.elapsed().as_millis() < 520 && *from != self.tab {
                anim = Anim::Page;
                layer = layer.kid(self.section_page(app, *from, &format!("tab-{}", tab_key(*from))).decorative().opacity(0.).ease(180));
            }
        }
        layer.kid(self.section_page(app, self.tab, &format!("tab-{name}")).anim(anim))
    }

    fn section_page(&self, app: &App, index: usize, key: &str) -> El {
        let _ = view_w(app);
        let body = if index == 0 { self.home_view(app) } else { self.browse_view(app, tab_key(index)) };
        el().key(key).cover().clip().kid(body)
    }

    /// The rail down the left, icons only: the zf mark; the sections, search; downloads, history and your picture at
    /// its foot. Resting on one names it, in a little label beside it.
    fn rail_view(&self, app: &App) -> El {
        let height = app.kit.height;
        let accent = crate::theme::accent();
        let span = RAIL;
        let inner = span - 2. * RAIL_PAD;
        // (a short window: the rows a touch shorter and closer, so sections and the foot never run into each other;
        // shorter still, the rail scrolls)
        let compact = height < t::K_SIDEBAR_SHORT_WINDOW;
        let (item_h, item_gap) = if compact { (t::K_SIDEBAR_ITEM_COMPACT, t::SPACE_2) } else { (t::K_SIDEBAR_ITEM, t::SPACE_6) };
        let names: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let item = |key: &str, glyph: &'static str, label: &str, on: bool, f: Rc<dyn Fn(&mut App)>| {
            let hot = app.kit.hover_within(k(key));
            names.borrow_mut().push((key.to_string(), label.to_string()));
            tap(
                col().key(key).w(inner).h(item_h).radius(if compact { t::K_SIDEBAR_CORNER_COMPACT } else { t::K_SIDEBAR_CORNER }).center()
                    .bg(if on { alpha(accent, 0.16) } else if hot { w(t::W07) } else { 0 })
                    .kid(icon(glyph, t::K_SIDEBAR_ICON, if on { accent } else if hot { t::C_WHITE } else { w(t::W60) })),
                t::PRESS_TAB,
                move |a| f(a),
            )
        };
        // (the sections, the rail's pages and their names, as the core's navigation has them)
        let nav = self.nav.view();
        let entry = |route: &CoreRoute| nav.foot.iter().chain(nav.pages.iter()).chain([&nav.search]).find(|e| &e.route == route);
        let on = |route: &CoreRoute| entry(route).is_some_and(|e| e.on);
        let label = |route: &CoreRoute| entry(route).map(|e| e.label.clone()).unwrap_or_default();
        let mut sections = col().gap(item_gap);
        for (i, tab) in nav.tabs.iter().enumerate() {
            sections = sections.kid(item(&format!("rail-{}", tab.key), glyph_icon(tab.glyph), &tab.label, tab.on, Rc::new(move |a: &mut App| {
                Pages::open_tab(a, i);
            })));
        }
        // (Manhwa, as the phone's tab: the shelf, a series and the reader are its own pages)
        sections = sections.kid(item("rail-manhwa", "book_open", &label(&CoreRoute::Manhwa), on(&CoreRoute::Manhwa), Rc::new(|a: &mut App| a.pages.open_manhwa())));
        sections = sections.kid(el().w(inner).h(t::S_HAIRLINE).bg(FLAT_LINE).margin(0., if compact { t::SPACE_4 } else { t::SPACE_8 }, 0., if compact { t::SPACE_4 } else { t::SPACE_8 }))
            .kid(item("rail-search", "search", &label(&CoreRoute::Search), on(&CoreRoute::Search), Rc::new(|a: &mut App| a.pages.open_search(&mut a.kit))));
        // (the rail's pages open fresh from wherever you are, never stacked on each other)
        let mut foot = col().gap(item_gap)
            .kid(item("rail-downloads", "download", &label(&CoreRoute::Downloads), on(&CoreRoute::Downloads), Rc::new(|a: &mut App| { a.pages.clear_for(); a.pages.push(Route::Downloads) })))
            .kid(item("rail-history", "history", &label(&CoreRoute::History), on(&CoreRoute::History), Rc::new(|a: &mut App| { a.pages.clear_for(); a.pages.open_history() })))
            .kid(item("rail-tv", "tv", &self.your_tv.view().entry, on(&CoreRoute::YourTv), Rc::new(|a: &mut App| a.pages.open_your_tv())))
            .kid(item("rail-settings", "settings", &label(&CoreRoute::Settings), on(&CoreRoute::Settings), Rc::new(|a: &mut App| Pages::open_settings(a))))
            .maybe(self.donate.view().on.then(|| item("rail-donate", "heart", &self.donate.view().button, self.sheet() == Some(Sheet::Donate), Rc::new(|a: &mut App| a.pages.open_sheet(Sheet::Donate)))));
        let me = self.me.as_ref();
        names.borrow_mut().push(("rail-you".into(), nav.profiles.clone()));
        let you = tap(
            col().key("rail-you").w(inner).h(if compact { t::K_SIDEBAR_AVATAR_TILE_COMPACT } else { t::K_SIDEBAR_AVATAR_TILE }).radius(t::K_SIDEBAR_CORNER).center()
                .bg(if self.menu() || app.kit.hover_within(k("rail-you")) { w(t::W07) } else { 0 })
                .kid(avatar(me, t::K_SIDEBAR_AVATAR, t::K_SIDEBAR_AVATAR_RADIUS)),
            t::PRESS_AVATAR,
            |a| a.pages.toggle_menu(),
        );
        foot = foot.kid(el().w(inner).h(t::S_HAIRLINE).bg(FLAT_LINE).margin(0., t::SPACE_4, 0., t::SPACE_4)).kid(you);
        // ("f" lit in the page's own colour, as the phone has it: the glow's, softened toward white)
        let lit = self.glow_name().and_then(|name| self.glows.borrow().get(&name).map(|g| g.0)).filter(|c| *c != 0).map_or(crate::theme::flix(), |c| lerp(c, t::C_WHITE, 0.55));
        // (one column: the mark, the sections straight under it, room, the foot; never overlapping)
        let mut rail = el().key("rail").at(0., crate::WINDOW_STRIP, f32::NAN, 0.).w(span)
            .kid(col().key("rail-scroll").cover().scroll_y()
                .kid(col().w(span).min_h(height - crate::WINDOW_STRIP).no_shrink().pad4(RAIL_PAD, BRAND_TOP, RAIL_PAD, t::SPACE_14)
                    .kid(zf_mark(lit, inner).key("rail-zf").hover()
                        .opacity(if app.session.view().who_mark && self.who_leaving.is_some_and(|at| at.elapsed().as_secs_f32() * 1000. < MORPH_MS) { 0. } else { 1. }).ease(140))
                    .kid(sections.margin(0., if compact { t::SPACE_12 } else { t::K_SIDEBAR_SECTIONS_TOP }, 0., 0.))
                    .kid(el().grow().min_h(t::SPACE_14))
                    .kid(foot)));
        // the name of the one under the pointer, in a little label beside it (over the page, never clipped by the rail)
        if !self.menu() {
            let names = names.borrow();
            if let Some((key, label)) = names.iter().find(|(key, _)| app.kit.hover_within(k(key))) {
                if let Some((x, y, w_, h_)) = app.kit.rect_of(k(key)) {
                    let tip = row().key(&format!("{key}-tip")).at(x + w_ + t::SPACE_10, y - crate::WINDOW_STRIP + h_ / 2. - t::K_TOOLTIP_HEIGHT / 2., f32::NAN, f32::NAN).h(t::K_TOOLTIP_HEIGHT)
                        .pad_xy(t::K_TOOLTIP_PAD, 0.).radius(t::R_SMALL).items_center().bg(t::C_TOOLTIP).decorative().anim(Anim::Fade(140))
                        .kid(line(label, t::T_TOOLTIP, Font::Medium, t::C_WHITE));
                    rail = rail.kid(tip);
                }
            }
        }
        rail
    }

    /// The profile's menu: a small card of glass growing out of the picture's corner.
    fn menu_view(&self, app: &App) -> Option<El> {
        let open = self.menu();
        let nav = self.nav.view();
        if !open && self.menu_closed.is_none() {
            return None;
        }
        // Your profiles, as Discord's account switcher: every one (picture and name), yours with a check, a click on
        // another switches to it; then Manage profiles and Add profile. Nothing else: the rest is in the rail.
        let row_ = |key: String, hot: bool| row().key(&key).fill_w().h(t::K_SIDEBAR_MENU_ROW).pad_xy(t::SPACE_10, 0.).radius(t::K_SIDEBAR_MENU_ROW_CORNER).items_center().bg(if hot { w(t::W07) } else { 0 });
        let mut card = ds::menu_card(t::K_SIDEBAR_MENU)
            .kid(text(&nav.profiles, ts(t::T_SIDEBAR_MENU_HEAD, Font::Bold, w(t::W50), 1)).margin(t::SPACE_10, t::SPACE_4, 0., t::SPACE_6));
        for p in &self.profiles {
            let mine = self.me.as_ref().is_some_and(|m| m.id == p.id);
            let key = format!("menu-as-{}", p.id);
            let hot = app.kit.hover_within(k(&key));
            let chosen = p.clone();
            card = card.kid(tap(
                row_(key, hot)
                    .kid(avatar(Some(p), t::K_SIDEBAR_MENU_AVATAR, t::K_SIDEBAR_MENU_AVATAR_RADIUS))
                    .kid(text(&p.name, ts(t::T_SIDEBAR_ITEM, if mine { Font::Bold } else { Font::Medium }, t::C_WHITE, 1)).grow().margin(t::SPACE_12, 0., 0., 0.))
                    .maybe(mine.then(|| icon("check", t::K_SIDEBAR_MENU_ICON, t::C_WHITE))),
                t::PRESS_ROW,
                move |a| {
                    a.pages.close_menu();
                    if !mine {
                        // (as picking on Who's watching: it opens with this one picked, shining, then Home; opened by
                        // follow_session with the pick already in, or its first frame showed everyone, then faded them)
                        Self::choose_profile(a, chosen.clone());
                    }
                },
            ));
        }
        // (the profiles, to switch: managing them is in Settings); then signing out, under a line
        let out_hot = app.kit.hover_within(k("menu-sign-out"));
        card = card.kid(ds::divider().margin(t::SPACE_6, t::SPACE_6, t::SPACE_6, t::SPACE_6))
            .kid(tap(
                row_("menu-sign-out".into(), out_hot)
                    .kid(col().size(t::K_SIDEBAR_MENU_AVATAR, t::K_SIDEBAR_MENU_AVATAR).center().kid(icon("log_out", t::K_SIDEBAR_MENU_ICON, w(t::W80))))
                    .kid(line(&nav.sign_out, t::T_SIDEBAR_ITEM, Font::Medium, t::C_WHITE).margin(t::SPACE_12, 0., 0., 0.)),
                t::PRESS_ROW,
                |a| { a.pages.close_menu(); Pages::sign_out_now(a) },
            ));
        // (beside your chip at the foot of the rail, growing out of it)
        let placed = card.at(RAIL + t::SPACE_8, f32::NAN, f32::NAN, t::SPACE_14);
        if open {
            let mut dismiss = el().cover().click(|a| a.pages.close_menu());
            dismiss.hand = false;
            Some(el().key("menu-layer").cover().kid(dismiss).kid(placed.key("menu").anim(Anim::Grow(0., 1.))))
        } else {
            Some(el().key("menu-leaving").cover().decorative().kid(placed.key("menu-out").anim(Anim::Shrink(0., 1.))))
        }
    }

    /// A GlassSheet: the page dimmed, and a card of glass from the top (anything with typing) that springs in.
    fn sheet_view(&self, app: &App) -> Option<El> {
        let (sheet, leaving) = match (self.sheet(), self.sheet_closed) {
            (Some(s), _) => (s, false),
            (None, Some((s, _))) => (s, true),
            _ => return None,
        };
        let width = (app.kit.width - t::S_SHEET_MARGIN * 2.).min(t::K_PAGE_DIALOG);
        let inner = width - t::SPACE_28 * 2.;   // (the card's own padding a side: fields and buttons the same width)
        let body = match sheet {
            Sheet::Profile => self.profile_sheet(app, inner),
            Sheet::Tv => self.tv_sheet(app, inner),
            Sheet::TvCaptions => self.tv_options_sheet(app, true),
            Sheet::TvSettings => self.tv_options_sheet(app, false),
            Sheet::Donate => self.donate_sheet(app),
        };
        let card = ds::sheet_card(width, body);
        // (it floats: the page behind blurred and dimmed. Closing, the very same card and dim play their way in
        // backwards: nothing is built again for the way out)
        let lifted = el().key(&format!("sheet-{sheet:?}"))
            .kid(card)
            .anim(if leaving { Anim::PageOut } else { Anim::Page });
        let placed = col().cover().center().kid(lifted);
        let mut dim = ds::sheet_backdrop().key("sheet-dim").anim(if leaving { Anim::FadeOut(200) } else { Anim::Fade(200) });
        if leaving {
            return Some(el().key("sheet-layer").cover().decorative().kid(dim).kid(placed));
        }
        dim = dim.click(|a| a.pages.close_sheet());
        dim.hand = false;
        Some(el().key("sheet-layer").cover().kid(dim).kid(placed))
    }

    /// Who's watching: the profiles in a line as big rounded pictures with their names, coming up one after another;
    /// under the pointer one grows a little and rings in the accent, its name brightening (and easing back as the
    /// pointer leaves). Picked, it stays as the others fade. Manage profiles fades a pencil onto each; a tap opens its
    /// small edit card. Add profile is always the last.
    /// Who's watching; `leaving`: the copy fading away over Home (its page doesn't zoom in again).
    fn who_view(&self, app: &App, leaving: bool) -> El {
        let (width, height) = (app.kit.width, app.kit.height);
        let accent = crate::theme::accent();
        // (only the main profile adds and manages, while there's room; the server holds to it too)
        let session = app.session.view();
        let in_main = session.can_manage;
        let can_add = session.can_add;
        let live = self.profiles.len() + usize::from(can_add);
        let count = if self.picking.is_some() { self.who_count.get().unwrap_or(live) } else { live };
        self.who_count.set(self.picking.is_some().then_some(count));
        let tile = ((width - t::K_SIGN_IN_WHO_ROOM) / count.max(1) as f32 - t::SPACE_28).clamp(t::K_SIGN_IN_FACE_MIN, t::K_SIGN_IN_FACE);
        let radius = tile * t::K_SIGN_IN_FACE_RADIUS_RATIO;
        let managing = session.managing;
        // (the pick straight from the core, as the phone draws it: the frame after the click, not the timer's next beat
        // (on opening, busy loading Home, that came late and the shine was cut short); the copy kept here holds it
        // while Who's watching fades over Home, after the core has let it go)
        let picked = session.picking.clone().or_else(|| self.picking.clone());
        let picking = picked.as_deref();
        // (each in its own place along the line, so the picked one can glide to the middle as the others fade)
        let gap = t::K_SIGN_IN_FACE_GAP;
        let line_w = count as f32 * tile + (count as f32 - 1.).max(0.) * gap;
        let place = |i: usize, chosen: bool| if chosen { (line_w - tile) / 2. } else { i as f32 * (tile + gap) };
        let mut tiles = el().w(line_w).h(tile + t::K_SIGN_IN_FACE_LABEL);
        for (i, p) in self.profiles.iter().enumerate() {
            let key = format!("who-{}", p.id);
            let hot = app.kit.hover_within(k(&key)) && picking.is_none();
            let chosen = picking == Some(p.id.as_str());
            let mut picture = el().size(tile, tile).radius(radius)
                .kid(avatar(Some(p), tile, radius))
                .kid(el().cover().decorative().radius(radius).bg(black(0.5)).center().kid(icon("pencil", tile * t::K_SIGN_IN_FACE_PENCIL_RATIO, t::C_WHITE))
                    .opacity(if managing { 1. } else { 0. }).ease(220))
                .kid(el().cover().decorative().radius(radius).border(t::K_SIGN_IN_FACE_RING, accent).opacity(if hot || chosen { 1. } else { 0. }).ease(200));
            // (the shine carries on into the copy fading over Home: on opening, Home's first load can hold the pick up,
            // and cut there it barely showed)
            if chosen {
                // (a light sweeping across it, and a little bump: it's been picked)
                // (one bright sweep, left to right, done before Home comes in)
                picture = picture.kid(el().cover().decorative().radius(radius).clip()
                    .kid(el().key(&format!("{key}-shine")).at(tile * t::K_SIGN_IN_SHINE_START_RATIO, 0., f32::NAN, 0.).w(tile * t::K_SIGN_IN_SHINE_WIDTH_SHARE).decorative()
                        .paint(Fill::Linear(0., 0., 1., 0., vec![(0., 0), (0.5, w(t::W60)), (1., 0)])).anim(Anim::Shift(-tile * t::K_SIGN_IN_SHINE_TRAVEL_RATIO, 620))));
                picture = el().key(&format!("{key}-picked")).anim(if self.who_picked { Anim::None } else { Anim::Pop(0.86, tile / 2.) }).kid(picture);
            }
            let person = p.clone();
            let face = tap(
                col().key(&key).w(tile).items_center()
                    .kid(picture.grow_hover(1.06))
                    .kid(text(&p.name, centred(ts(t::T_ENTRY_NAME, Font::Medium, t::C_WHITE, 1))).fill_w().margin(0., t::SPACE_12, 0., 0.)
                        .opacity(if hot || chosen { 1. } else { 0.62 }).ease(200)),
                t::PRESS_DEFAULT,
                move |a| Self::choose_profile(a, person.clone()),
            );
            // (the rest fading once one is picked)
            // (a switch from the menu: the row's width settles as the profile changes (only the main one adds), so the
            // picked one, in the middle all along, doesn't slide with it)
            let glide = if self.who_picked { 0 } else { 460 };
            tiles = tiles.kid(col().key(&format!("{key}-slot")).at(place(i, chosen), 0., f32::NAN, f32::NAN).slide(glide)
                .opacity(if picking.is_some() && !chosen { 0. } else { 1. }).ease(260).kid(face));
        }
        if can_add {
            let hot = app.kit.hover_within(k("who-add")) && picking.is_none();
            tiles = tiles.kid(col().key("who-add-slot").at(place(self.profiles.len(), false), 0., f32::NAN, f32::NAN)
                .opacity(if picking.is_some() { 0. } else { 1. }).ease(260)
                .kid(tap(
                    col().key("who-add").w(tile).items_center()
                        .kid(col().size(tile, tile).radius(radius).bg(t::C_ADD_TILE).border(t::K_SIGN_IN_ADD_TILE_BORDER, w(t::W16)).center()
                            .kid(icon("plus", tile * t::K_SIGN_IN_FACE_ADD_RATIO, w(t::W70)))
                            .kid(el().cover().decorative().radius(radius).border(t::K_SIGN_IN_FACE_RING, accent).opacity(if hot { 1. } else { 0. }).ease(200))
                            .grow_hover(1.06))
                        .kid(text(&session.who_add, centred(ts(t::T_ENTRY_NAME, Font::Medium, t::C_WHITE, 1))).fill_w().margin(0., t::SPACE_12, 0., 0.)
                            .opacity(if hot { 1. } else { 0.62 }).ease(200)),
                    t::PRESS_DEFAULT,
                    |a| a.session.send(zcore::session::Event::Add),
                )));
        }
        // the button under them: Manage profiles, or Done while managing (each fading in as it changes)
        // (no button: its room kept all the same, so the profiles never move when it comes or goes, as when a switch
        // makes another profile the one here)
        let button = if !in_main && !managing { el().h(ds::Size::L.h()) } else if managing {
            ds::button(app, "who-done", ds::Kind::Primary, ds::Size::L, Some("check"), &session.who_done, |a| a.session.send(zcore::session::Event::Manage { on: false })).anim(Anim::Fade(220))
        } else {
            ds::button(app, "who-manage", ds::Kind::Secondary, ds::Size::L, Some("pencil"), &session.who_manage, |a| a.session.send(zcore::session::Event::Manage { on: true }))
                .anim(if picking.is_some() { Anim::None } else { Anim::Fade(220) })
        };
        let heading = &session.who_heading;
        let mut c = col().items_center()
            // (switching, it isn't there, and fading if a pick's made here; its room kept)
            .kid(text(heading, centred(ts(t::T_WHO_TITLE_TV, Font::Bold, t::C_WHITE, 1))).key(&format!("who-heading-{managing}"))
                .opacity(if picking.is_some() { 0. } else { 1. }).ease(220))
            .kid(tiles.margin(0., t::SPACE_40, 0., 0.))
            .kid(button.opacity(if picking.is_some() { 0. } else { 1. }).ease(220).margin(0., t::SPACE_48, 0., 0.));
        if !self.note.is_empty() {
            c = c.kid(ds::error_text(&self.note, true).margin(0., t::SPACE_16, 0., 0.));
        }
        // behind it, just the app's own grey-to-black gradient (the same one Home is on)
        // (the whole screen fades in, nothing moving, as it fades out over Home)
        let opening = if leaving { Anim::None } else { Anim::Fade(i64::from(t::D_SETTLE)) };
        el().key("who").size(width, height).bg(PAGE).clip().anim(opening).kid(page_ground("who-ground", 0.))
            // (the name where the rail's mark is; as Who's watching gives way to Home it becomes the rail's zf; read
            // straight from the core, so a switch from the menu never shows it for a frame)
            .maybe((self.who_leaving.is_none() && session.who_mark).then(|| col().at(BRAND_X, crate::WINDOW_STRIP + BRAND_TOP, f32::NAN, f32::NAN).kid(brand_mark(crate::theme::flix(), None))))
            .kid(col().cover().center().kid(c))
    }

    /// A profile's small edit card: its picture (tap to choose another), its name, and (managing) deleting it.
    fn profile_sheet(&self, app: &App, width: f32) -> El {
        let edit = self.editor.view();
        let me = if self.adding { None } else { self.editing.as_ref().or(self.me.as_ref()) };
        let shown_me = me.map(|m| Me { avatar: if edit.removed { None } else { m.avatar.clone() }, ..m.clone() });
        let picture = match &self.avatar {
            Some(path) => col().size(t::K_SETTINGS_PROFILE_PICTURE, t::K_SETTINGS_PROFILE_PICTURE).radius(t::K_SHELL_SHEET_PICTURE_RADIUS).clip().bg(t::C_BG).image(format!("file:{path}")),
            None => avatar(shown_me.as_ref(), t::K_SETTINGS_PROFILE_PICTURE, t::K_SHELL_SHEET_PICTURE_RADIUS),
        };
        let mut c = col().fill_w().items_center()
            .kid(text(&edit.title, centred(ts(t::T_SHEET_TITLE, Font::Bold, t::C_WHITE, 1))).fill_w());
        // (a picture just picked: in its cropper, moved and zoomed to fit, and another a click away)
        c = if self.avatar.is_some() {
            c.kid(self.crop_editor(app, t::K_SHELL_CROP_SIDE, t::K_SHELL_CROP_RADIUS).margin(0., t::K_SHELL_SHEET_GAP, 0., 0.))
                .kid(ds::button(app, "choose-another", ds::Kind::Ghost, ds::Size::S, None, &edit.words.choose_another, Self::change_avatar).margin(0., t::SPACE_6, 0., 0.))
        } else {
            c.kid(tap(
                el().key("choose-picture").size(t::K_SETTINGS_PROFILE_PICTURE, t::K_SETTINGS_PROFILE_PICTURE).margin(0., t::K_SHELL_SHEET_GAP, 0., 0.)
                    .kid(picture)
                    .kid(col().size(t::K_SHELL_EDIT_BADGE, t::K_SHELL_EDIT_BADGE).radius(t::K_SHELL_EDIT_BADGE / 2.).bg(t::C_INK).center().at(f32::NAN, f32::NAN, -t::SPACE_4, -t::SPACE_4).kid(icon("pencil", t::K_SHELL_EDIT_BADGE_ICON, t::C_DARK))),
                t::PRESS_DEFAULT,
                Self::change_avatar,
            ))
        };
        if edit.can_remove {
            c = c.kid(ds::danger(app, "remove-picture", ds::Size::S, None, &edit.words.remove_picture, &edit.words.remove_ask, edit.removing,
                |a| {
                    if a.pages.editor.view().removing {
                        a.pages.avatar = None;
                    }
                    a.pages.editor.send(zcore::profiles::EditEvent::Remove);
                }).margin(0., t::SPACE_14, 0., 0.));
        }
        let save: Cb = Rc::new(Self::save_profile);
        let limit: Cb = Rc::new(|a: &mut App| {
            let current = a.kit.text_of("profile-name");
            let clamped = zcore::profiles::clamp_name(&current);
            if clamped != current {
                a.kit.set_text("profile-name", &clamped);
            }
        });
        c = c.kid(line(&edit.words.name, t::T_FIELD_LABEL, Font::Bold, t::C_WHITE).fill_w().margin(t::SPACE_4, t::K_SHELL_SHEET_GAP, 0., t::SPACE_8))
            .kid(glass_field(app, Field { key: "profile-name", hint: &edit.words.name_hint, change: Some(limit), submit: Some(save.clone()), ..Default::default() }, width, None));
        // the footer, well apart from the name: Delete on the left (never for the main profile: it's kept, as
        // Netflix, Disney+ and Max keep theirs), or Cancel when adding; Save on the right
        let left: El = if let Some(asking) = edit.delete {
            ds::danger(app, "delete-profile", ds::Size::L, Some("trash"), &edit.words.delete, &edit.words.delete_ask, asking, |a| a.pages.editor.send(zcore::profiles::EditEvent::Delete))
        } else if edit.cancel {
            ds::button(app, "profile-cancel", ds::Kind::Secondary, ds::Size::L, None, &edit.words.cancel, |a| a.pages.close_sheet())
        } else {
            el()
        };
        c = c.kid(el().fill_w().h(t::S_HAIRLINE).bg(w(t::W07)).margin(0., t::K_SHELL_SHEET_FOOT_TOP, 0., t::K_SHELL_SHEET_FOOT_GAP))
            .kid(row().fill_w().items_center()
                .kid(left)
                .kid(el().grow())
                .kid(ds::button(app, "profile-save", ds::Kind::Primary, ds::Size::L, None, &edit.button, move |a| save(a))));
        if let Some(note) = &edit.note {
            c = c.kid(ds::error_text(note, true).fill_w().margin(0., t::SPACE_12, 0., 0.));
        }
        c.pad4(0., 0., t::SPACE_8, t::SPACE_4)
    }

    /// Sign in a TV (`zcore::tv_link::SignInTv`): the code on the TV's sign-in screen signs it in; or the steps to get
    /// the app onto it.
    fn tv_sheet(&self, app: &App, width: f32) -> El {
        use zcore::tv_link::{SignInEvent, Stage};
        let v = self.sign_in_tv.view();
        let stage = match v.stage { Stage::Code => "code", Stage::Steps => "steps", Stage::Done => "done" };
        let tile = col().size(t::K_YOUR_TV_SIGN_IN_TILE, t::K_YOUR_TV_SIGN_IN_TILE).radius(t::R_ICON_TILE).bg(ds::surface(3)).center().margin(0., 0., 0., t::SPACE_16)
            .kid(icon(if v.stage == Stage::Done { "check" } else { "tv" }, t::K_YOUR_TV_SIGN_IN_ICON, t::C_WHITE));
        let mut c = col().key(&format!("tv-{stage}")).fill_w().items_center().anim(Anim::Rise(t::SPACE_10, t::D_MOVE as i64))
            .kid(tile)
            .kid(text(&v.title, centred(ts(t::T_TV_TITLE, Font::Bold, t::C_WHITE, 2))).fill_w())
            .kid(text(&v.line, centred(ts(t::T_PHONE_TEXT, Font::Sans, w(t::W85), 2))).fill_w().margin(0., t::SPACE_6, 0., t::SPACE_24));
        match v.stage {
            Stage::Done => c = c.kid(ds::button(app, "tv-done", ds::Kind::Primary, ds::Size::L, None, &v.done, |a| a.pages.close_sheet()).fill_w()),
            Stage::Steps => {
                for step in &v.steps {
                    let words = if step.bold.is_empty() {
                        text(&step.before, ts(t::T_BODY, Font::Sans, t::C_WHITE, 0)).grow().margin(t::SPACE_12, 0., 0., 0.)
                    } else {
                        // (the address in bold: what to type)
                        row().grow().margin(t::SPACE_12, 0., 0., 0.)
                            .kid(line(&step.before, t::T_BODY, Font::Sans, t::C_WHITE))
                            .kid(line(&step.bold, t::T_BODY, Font::Bold, t::C_WHITE))
                            .kid(line(&step.after, t::T_BODY, Font::Sans, t::C_WHITE))
                    };
                    c = c.kid(el().fill_w().h(t::S_HAIRLINE).bg(t::C_LINE))
                        .kid(row().fill_w().pad_xy(0., t::SPACE_12).items_start()
                            .kid(col().size(t::K_YOUR_TV_STEP, t::K_YOUR_TV_STEP).no_shrink().radius(t::R_STEP_TILE).bg(ds::surface(3)).center()
                                .kid(line(&step.number, t::T_META, Font::Medium, t::C_WHITE)))
                            .kid(words));
                }
                c = c.kid(ds::button(app, "tv-back", ds::Kind::Secondary, ds::Size::L, None, &v.back, |a| a.pages.sign_in_tv.send(SignInEvent::Back))
                    .fill_w().margin(0., t::SPACE_10, 0., 0.));
            }
            Stage::Code => {
                let go: Cb = Rc::new(Self::sign_in_tv);
                let upper: Cb = Rc::new(|a: &mut App| {
                    let typed = a.kit.text_of("tv-code");
                    let clean = zcore::tv_link::clean_code(&typed);
                    if clean != typed { a.kit.set_text("tv-code", &clean); }
                    a.pages.sign_in_tv.send(SignInEvent::Typed);
                });
                c = c.kid(glass_field(app, Field { key: "tv-name", hint: &v.name_hint, submit: Some(go.clone()), ..Default::default() }, width, None).margin(0., 0., 0., t::SPACE_10))
                    .kid(glass_field(app, Field { key: "tv-code", hint: &v.code_hint, change: Some(upper), submit: Some(go.clone()), ..Default::default() }, width, None).margin(0., 0., 0., t::SPACE_10))
                    .kid(ds::button(app, "tv-sign-in", ds::Kind::Primary, ds::Size::L, None, &v.button, move |a| go(a)).fill_w());
                if let Some(error) = &v.error {
                    c = c.kid(ds::error_text(error, true).fill_w().margin(0., t::SPACE_10, 0., 0.));
                }
                c = c.kid(tap(
                    row().key("tv-how").margin(0., t::SPACE_16, 0., 0.)
                        .kid(line(&v.no_app, t::T_NOTE, Font::Sans, w(t::W85)))
                        .kid(col().kid(line(&v.how, t::T_NOTE, Font::Medium, t::C_WHITE)).kid(el().fill_w().h(t::S_HAIRLINE).bg(t::C_WHITE))),
                    1.,
                    |a| a.pages.sign_in_tv.send(SignInEvent::Steps),
                ));
            }
        }
        c.pad4(0., t::SPACE_10, t::SPACE_8, t::SPACE_4)
    }

    /// Donations (`zcore::donate`): the title and line, the goal as a bar with its words under it, each bank detail
    /// (its label small, its value bold) with a Copy button, and Done.
    fn donate_sheet(&self, app: &App) -> El {
        let v = self.donate.view();
        let mut c = col().fill_w().items_center()
            .kid(wordmark(t::T_LOGIN_WORDMARK.0, crate::theme::flix()))
            .kid(text(&v.line, centred(ts(t::T_NOTE, Font::Medium, w(t::W60), 2))).fill_w().margin(0., t::SPACE_6, 0., 0.));
        // (the goal as a meter: what's in and of how much on one line, the bar under them)
        if let Some(goal) = &v.goal {
            c = c.kid(row().fill_w().items_end().margin(0., t::SPACE_16, 0., 0.)
                    .kid(line(&goal.raised, t::T_BODY, Font::Bold, t::C_WHITE))
                    .kid(el().grow())
                    .kid(line(&goal.of, t::T_NOTE, Font::Medium, w(t::W60))))
                .kid(el().fill_w().h(t::K_DONATE_BAR).radius(t::K_DONATE_BAR / 2.).clip().bg(w(t::W10)).margin(0., t::SPACE_8, 0., 0.)
                    .kid(el().pct_w(goal.share).h(t::K_DONATE_BAR).radius(t::K_DONATE_BAR / 2.).bg(crate::theme::accent())));
        }
        c = c.kid(ds::button_onward(app, "donate-bank", ds::Kind::Secondary, ds::Size::L, "chevron_down", if v.bank_open { 180. } else { 0. }, &v.bank,
            |a| a.pages.donate.send(zcore::donate::Event::Bank)).fill_w().margin(0., t::SPACE_16, 0., 0.));
        // (as the website's: the details' height eases open and shut over D_FOLD, their own height measured last frame)
        let was = self.donate_fold.get().0;
        if was != v.bank_open {
            self.donate_fold.set((v.bank_open, Some(Instant::now())));
        }
        let since = self.donate_fold.get().1;
        let eased = since.map_or(1., |at| crate::kit::bezier(t::EASE_EASE, (at.elapsed().as_secs_f32() * 1000. / t::D_FOLD as f32).min(1.)));
        let shown = if v.bank_open { eased } else { 1. - eased };
        {
            let detail = |d: &zcore::donate::Detail| col()
                .kid(text(&d.label, ts(t::T_NOTE, Font::Sans, w(t::W60), 1)))
                .kid(text(&d.value, ts(t::T_BODY, Font::Bold, t::C_WHITE, 3)).margin(0., t::K_DONATE_LABEL_GAP, 0., 0.));
            // (the names side by side in one row; each number a row of its own, with Copy)
            // (laid at its own height inside the fold, however short the fold is just now, so it can be measured)
            let mut bank = col().key("donate-details").at(0., 0., 0., f32::NAN).pad4(0., t::SPACE_6, 0., 0.);
            let names: Vec<_> = v.details.iter().filter(|d| d.name).collect();
            if !names.is_empty() {
                bank = bank.kid(row().fill_w().gap(t::SPACE_12).pad_xy(0., t::SPACE_8).kids(names.into_iter().map(|d| detail(d).grow()).collect()));
            }
            for (i, d) in v.details.iter().enumerate().filter(|(_, d)| !d.name) {
                let value = d.value.clone();
                let copy = d.copyable.then(|| ds::icon_button(app, &format!("donate-copy-{i}"), ds::Kind::Secondary, ds::Size::S, if d.copied { "check" } else { "copy" }, move |a| {
                    a.window.set_clipboard(&value.encode_utf16().collect::<Vec<_>>());
                    a.pages.donate.send(zcore::donate::Event::Copied { index: i as u32 });
                }).margin(t::SPACE_12, 0., 0., 0.));
                bank = bank.kid(el().fill_w().h(t::S_HAIRLINE).bg(t::C_LINE))
                    .kid(row().fill_w().pad_xy(0., t::SPACE_8).items_center().kid(detail(d).grow()).maybe(copy));
            }
            let full = app.kit.rect_of(k("donate-details")).map_or(0., |r| r.3);
            c = c.kid(el().key("donate-fold").fill_w().h((full * shown).round()).clip().kid(bank));
        }
        c.pad4(0., t::SPACE_10, t::SPACE_8, t::SPACE_4)
    }

    /// A new version out (the core's updater says when, and its words): a small card at the bottom right, its icon, the
    /// version, Update (now, in the bootstrapper) and a cross to put it away (it updates as zunflix next opens anyway;
    /// a forced update has no cross).
    fn update_toast(&self, app: &App) -> Option<El> {
        let live = app.updater.view().card.clone();
        // (put away: the card it showed slides off the way it came, then it's gone)
        if live.is_some() {
            *self.toast_card.borrow_mut() = live.clone();
            *self.toast_gone.borrow_mut() = None;
        } else if self.toast_card.borrow().is_some() {
            let gone = *self.toast_gone.borrow_mut().get_or_insert_with(Instant::now);
            if gone.elapsed().as_millis() > 500 {
                *self.toast_card.borrow_mut() = None;
            }
        }
        let card = self.toast_card.borrow().clone();
        let Some(card) = card.filter(|_| self.sheet().is_none()) else {
            *self.toast_at.borrow_mut() = None;
            return None;
        };
        let away = live.is_none();
        // (in: drawn first just off the right edge, then where it rests, so it slides there; away: back off the edge)
        let shown = *self.toast_at.borrow_mut().get_or_insert_with(Instant::now);
        let out = shown.elapsed().as_millis() < 40 || away;
        let close = tap(
            col().key("update-hide").size(t::K_SHELL_TOAST_CLOSE, t::K_SHELL_TOAST_CLOSE).radius(t::K_SHELL_TOAST_CLOSE / 2.).center()
                .bg(if app.kit.hover_within(k("update-hide")) { w(t::W10) } else { 0 })
                .kid(icon("x", t::K_SHELL_TOAST_CLOSE_ICON, w(t::W70))),
            t::PRESS_ROUND,
            |a| a.updater.send(zcore::update::Event::Later),
        );
        // (a light rim, so it stands out from the page under it)
        let card = panel(row().key("update-toast").w(t::K_SIGN_IN_UPDATE_CARD).pad4(t::SPACE_14, t::SPACE_14, t::SPACE_12, t::SPACE_14).items_center().gap(t::SPACE_12), ds::R_LG)
            .border(t::S_HAIRLINE, w(t::W22))
            .kid(el().size(t::K_SETTINGS_APP_ICON, t::K_SETTINGS_APP_ICON).no_shrink().image("res:icon"))
            .kid(col().grow()
                .kid(line(&card.title, t::T_SIGN_IN_UPDATE_NAME, Font::Bold, t::C_WHITE))
                .kid(line(&card.line, t::T_NOTE, Font::Medium, w(t::W60)).margin(0., t::SPACE_2, 0., 0.)))
            .kid(ds::button(app, "update-go", ds::Kind::Primary, ds::Size::S, None, &card.button, |a| a.updater.send(zcore::update::Event::Update)))
            // (a forced update has no way to put it away)
            .maybe((!card.forced).then_some(close));
        // (in: sliding in as it fades in; away: the way in backwards, the same time, off the edge, no fading)
        let placed = card.at(f32::NAN, f32::NAN, if out { -t::K_SHELL_TOAST_AWAY } else { t::K_SHELL_TOAST_EDGE }, t::K_SHELL_TOAST_EDGE);
        Some(if away {
            placed.slide_away(460)
        } else {
            placed.slide(460).opacity(if out { 0. } else { 1. }).ease(320)
        })
    }

    /// zunflix's posters as a wall turned -10°, the columns drifting up and down past each other.
    fn poster_wall(&self, key: &str, width: f32, height: f32) -> El {
        let mut wall = el().key(key).cover().clip();
        if !self.wall.is_empty() {
            // 4 columns on a phone; more as the window widens (each about 150 across), turned -10° and a little bigger
            let span = width * t::K_SIGN_IN_WALL_SPAN_RATIO;
            let columns = (span / t::K_SIGN_IN_WALL_COLUMN).ceil().max(4.) as usize;
            let card = (span - t::S_CARD_GAP * (columns as f32 - 1.)) / columns as f32;
            let tall = card * t::K_SIGN_IN_WALL_POSTER_RATIO;
            let cycle = (tall + t::S_CARD_GAP) * 6.;
            wall = wall.kid(
                row().key(&format!("{key}-turned")).w(span).h(height * t::K_SIGN_IN_WALL_HEIGHT_RATIO).at((width - span) / 2., -height * (t::K_SIGN_IN_WALL_HEIGHT_RATIO - 1.) / 2., f32::NAN, f32::NAN).gap(t::S_CARD_GAP).items_start().turn(-10.).decorative().kids(
                    (0..columns).map(|x| {
                        let pictures = (0..14).map(|y| {
                            // (each column its own six, never one another's: no poster twice on the wall)
                            let i = (x * 6 + y % 6) % self.wall.len();
                            el().size(card, tall).no_shrink().radius(t::R_WALL_POSTER).clip().bg(t::C_CARD).image(net::card_img(&self.wall[i], zcore::pictures::POSTER))
                        }).collect();
                        let dy = if x % 2 == 0 { -cycle } else { cycle };
                        col().key(&format!("{key}-{x}")).w(card).no_shrink().gap(t::S_CARD_GAP).kids(pictures).drift(dy, 60. + x as f32 * 4.)
                    }).collect(),
                ),
            );
        }
        wall
    }

    /// A new account waiting for the owner's approval (`zcore::session`'s Waiting), as the website's: the hourglass on
    /// its own, the words, and Sign out as a quiet link, on the sign-in card over the poster wall; the core goes in by
    /// itself once it's approved.
    fn waiting_view(&self, app: &App) -> El {
        let (width, height) = (app.kit.width, app.kit.height);
        let session = app.session.view();
        let wall = self.sign_wall(width, height);
        let card_w = (width - t::SPACE_16 * 2.).min(t::K_SIGN_IN_CARD);
        let card = col().key("waiting-card").w(card_w).radius(t::R_PANEL).clip().bg(FLAT_PANEL).items_center()
            .pad4(t::K_SIGN_IN_CARD_PAD, t::K_SIGN_IN_CARD_TOP, t::K_SIGN_IN_CARD_PAD, t::SPACE_24)
            .kid(icon("hourglass", t::K_YOUR_TV_TILE_BOX, t::C_WHITE).anim(Anim::Turn(i64::from(t::D_HOURGLASS_TURN))))
            .kid(text(&session.waiting_title, centred(ts(t::T_SHEET_TITLE, Font::Bold, t::C_WHITE, 1))).fill_w().margin(0., t::SPACE_16, 0., 0.))
            .kid(text(&session.waiting_line, centred(ts(t::T_SIGN_IN_LINE, Font::Medium, w(t::W60), 2))).fill_w().margin(0., t::SPACE_6, 0., 0.))
            .kid(ds::button(app, "waiting-sign-out", ds::Kind::Ghost, ds::Size::S, None, &session.waiting_sign_out,
                |a| a.session.send(zcore::session::Event::SignOut)).margin(0., t::SPACE_16, 0., 0.));
        el().cover().kid(wall).kid(col().cover().center().kid(card.anim(Anim::Rise(t::SPACE_10, i64::from(t::D_MOVE)))))
    }

    /// zunflix under maintenance (`zcore::session`'s Maintenance): the sign-in card over the poster wall, a gear on the
    /// empty states' badge, the title and the owner's message; once it's over, a tick, "We're back" and Continue.
    fn maintenance_view(&self, app: &App) -> El {
        let (width, height) = (app.kit.width, app.kit.height);
        let session = app.session.view();
        let wall = self.sign_wall(width, height);
        let card_w = (width - t::SPACE_16 * 2.).min(t::K_SIGN_IN_CARD);
        let over = session.maintenance_over;
        let badge = col().size(t::K_CONTROL_EMPTY_BADGE, t::K_CONTROL_EMPTY_BADGE).radius(t::K_CONTROL_EMPTY_BADGE / 2.).bg(t::C_EMPTY_BADGE).center()
            .kid(icon(if over { "check" } else { "settings" }, t::K_CONTROL_EMPTY_ICON, t::C_EMPTY_ICON));
        // (each look its own key: the card's insides cross-fade when maintenance ends)
        let inside = col().key(if over { "maintenance-back" } else { "maintenance-on" }).fill_w().items_center().anim(Anim::Fade(i64::from(t::D_FAST)))
            .kid(badge)
            .kid(text(&session.maintenance_title, centred(ts(t::T_SHEET_TITLE, Font::Bold, t::C_WHITE, 2))).fill_w().margin(0., t::SPACE_16, 0., 0.))
            .maybe((!session.maintenance_message.is_empty()).then(|| {
                text(&session.maintenance_message, centred(ts(t::T_SIGN_IN_LINE, Font::Medium, w(t::W85), 6))).fill_w().margin(0., t::SPACE_8, 0., 0.)
            }))
            .maybe((!session.maintenance_back.is_empty()).then(|| {
                text(&session.maintenance_back, centred(ts(t::T_SIGN_IN_LINE, Font::Medium, w(t::W60), 1))).fill_w().margin(0., t::SPACE_8, 0., 0.)
            }))
            .maybe(over.then(|| {
                crate::ds::button(app, "maintenance-continue", crate::ds::Kind::Primary, crate::ds::Size::L, None, &session.maintenance_continue,
                    |a| a.session.send(zcore::session::Event::Continue)).fill_w().margin(0., t::SPACE_24, 0., 0.)
            }));
        let card = col().key("maintenance-card").w(card_w).radius(t::R_PANEL).clip().bg(FLAT_PANEL).items_center()
            .pad4(t::K_SIGN_IN_CARD_PAD, t::K_SIGN_IN_CARD_TOP, t::K_SIGN_IN_CARD_PAD, t::K_SIGN_IN_CARD_PAD)
            .kid(inside);
        col().cover().bg(t::C_BG).kid(wall).kid(col().cover().center().kid(card.anim(Anim::Rise(t::SPACE_10, i64::from(t::D_MOVE)))))
    }

    /// The sign-in wall, as the website's: the posters fading in (signing out doesn't cut straight to it), softly
    /// blurred and tinted, shaded towards the foot, and darker round the edges (an ellipse a touch above the middle).
    fn sign_wall(&self, width: f32, height: f32) -> El {
        let shade = |a: f32| black(a) | 0x050505;
        self.poster_wall("wall", width, height).anim(Anim::Fade(i64::from(t::D_OPEN)))
            .kid(el().cover().decorative().blur(t::K_SIGN_IN_WALL_BLUR))
            .kid(el().cover().decorative().bg(t::C_WALL_TINT))
            .kid(el().cover().decorative().paint(Fill::Linear(0., 0., 0., 1., vec![(0., black(t::K_SIGN_IN_WALL_SHADE_TOP_SHARE)), (1., black(t::K_SIGN_IN_WALL_SHADE_FOOT_SHARE))])))
            .kid(el().cover().decorative().paint(Fill::Ellipse(0.5, 0.48, 0.7, 0.62, vec![(0., shade(t::K_SIGN_IN_WALL_EDGE_IN_SHARE)), (0.78, shade(t::K_SIGN_IN_WALL_EDGE_OUT_SHARE)), (1., shade(t::K_SIGN_IN_WALL_EDGE_OUT_SHARE))])))
    }

    /// How a part of the sign-in card comes in, as the website's: only as the card first shows (`entering`), the
    /// `n`th part after the one before (0 the card itself, 1 its tabs, then what's in it).
    fn part_in(entering: bool, n: u32) -> Anim {
        if !entering {
            return Anim::None;
        }
        match n {
            0 => Anim::Arrive { dy: t::K_SIGN_IN_CARD_RISE, from: t::K_SIGN_IN_CARD_FROM_SHARE, fade: i64::from(t::D_SIGN_IN_FADE), settle: i64::from(t::D_SIGN_IN_MOVE), after: 0 },
            1 => Anim::Arrive { dy: t::K_SIGN_IN_TAB_RISE, from: 1., fade: i64::from(t::D_SIGN_IN_TAB_FADE), settle: i64::from(t::D_SIGN_IN_TAB_MOVE), after: i64::from(t::D_SIGN_IN_TAB_AFTER) },
            n => Anim::Arrive { dy: t::K_SIGN_IN_PART_RISE, from: 1., fade: i64::from(t::D_SIGN_IN_TAB_FADE), settle: i64::from(t::D_SIGN_IN_TAB_MOVE),
                after: i64::from(t::D_SIGN_IN_PART_AFTER + t::D_SIGN_IN_PART_STEP * (n.min(4) - 2)) },
        }
    }

    /// Signing in: a wall of posters turned and drifting behind one panel of glass.
    fn login(&self, app: &App) -> El {
        let (width, height) = (app.kit.width, app.kit.height);
        let sign_in = &app.session.view().sign_in;
        let wall = self.sign_wall(width, height);
        let card_w = (width - t::SPACE_16 * 2.).min(t::K_SIGN_IN_CARD);
        let inner = card_w - t::K_SIGN_IN_CARD_PAD * 2.;
        let auth: Cb = Rc::new(Self::auth);
        let eye = || bare_icon_button("reveal", if self.reveal { "eye_off" } else { "eye" }, t::K_SIGN_IN_REVEAL, t::K_PAGE_FIELD_GLYPH, w(t::W80), |a| {
            a.pages.reveal = !a.pages.reveal;
            a.kit.focus = k("password");
        });
        use zcore::session::Mode;
        // (the core's step as it is now: the card changes with the click, not on the next tick)
        let mode = sign_in.mode;
        // (the card coming in: its parts one after another, for the moment it first shows)
        let entering = {
            let since = self.login_since.get().unwrap_or_else(|| { let now = Instant::now(); self.login_since.set(Some(now)); now });
            since.elapsed().as_millis() < 900
        };
        let part = |n: u32| Self::part_in(entering, n);
        // (the line under the name: the one it said fading out over the new one fading in, as it changes)
        let line_was = {
            let mut was = self.line_was.borrow_mut();
            if was.0 != sign_in.line {
                let old = std::mem::replace(&mut was.0, sign_in.line.clone());
                if !old.is_empty() {
                    was.2 += 1;
                    was.1 = old;
                    was.3 = Some(Instant::now());
                }
            }
            was.3.filter(|at| at.elapsed().as_millis() < 320).map(|_| (was.1.clone(), was.2))
        };
        // (the step's fields, as the core lists them; the first takes the focus)
        let wants = |field: &str| sign_in.fields.iter().any(|f| key_of(*f) == field);
        // (the name alone, centred)
        let words = |s: &str| text(s, centred(ts(t::T_SIGN_IN_LINE, Font::Medium, w(t::W85), 2))).fill_w();
        let mut form = col().fill_w().items_center()
            .kid(el().key("login-name").anim(part(2)).kid(wordmark(t::T_LOGIN_WORDMARK.0, crate::theme::flix())))
            // (the line under the name, the core's for the step, crossfading as it changes)
            .kid(el().key("login-line").fill_w().margin(0., t::SPACE_8, 0., 0.).anim(part(3))
                .kid(words(&sign_in.line).key(&format!("welcome-{mode:?}")).anim(Anim::Fade(300)))
                .maybe(line_was.map(|(old, n)| words(&old).at(0., 0., 0., f32::NAN).key(&format!("welcome-was-{n}")).anim(Anim::FadeOut(300)).decorative())));
        // (every field there, those the step doesn't take folded shut: a change of step opens and closes them)
        form = form.kid(el().h(t::K_SIGN_IN_FIELDS_TOP - t::SPACE_10).no_shrink());
        for (key, hint) in [("email", &sign_in.email_title), ("code", &sign_in.code_title), ("username", &sign_in.username_title), ("password", &sign_in.password_title)] {
            let open = self.fold(key, wants(key));
            // (a field folding away keeps the words it had while it showed)
            let hint = {
                let mut hints = self.fold_hints.borrow_mut();
                if wants(key) || !hints.contains_key(key) {
                    hints.insert(key, hint.clone());
                }
                hints[key].clone()
            };
            let hint = hint.as_str();
            let field = Field { key, hint, secret: key == "password" && !self.reveal, submit: Some(auth.clone()), ..Default::default() };
            let mut piece = glass_field(app, field, inner, (key == "password").then(eye)).margin(0., t::SPACE_10, 0., 0.);
            if open < 1. {
                piece = piece.decorative();
            }
            form = form.kid(el().key(&format!("fold-{key}")).w(inner).h((t::S_FIELD + t::SPACE_10) * open).no_shrink().clip().opacity(open).anim(part(4)).kid(piece));
        }
        // Remember me: ticked, this PC stays signed in; unticked, it's signed out when the app closes
        let remembering = self.fold("remember", sign_in.remember.is_some());
        if remembering > 0. {
            let on = self.remember;
            let hot = app.kit.hover_within(k("remember"));
            let tick = col().size(t::K_SIGN_IN_TICK, t::K_SIGN_IN_TICK).radius(t::K_SIGN_IN_TICK_RADIUS).center().no_shrink()
                .bg(if on { t::C_WHITE } else if hot { w(t::W10) } else { 0 }).border(if on { 0. } else { t::K_SIGN_IN_TICK_BORDER }, w(if hot { t::W50 } else { t::W32 }))
                .maybe(on.then(|| icon("check", t::K_SIGN_IN_TICK_ICON, t::C_BLACK)));
            // (Log in: Forgot password? across from it)
            let forgot = sign_in.forgot.then(|| link(app, "forgot", &sign_in.forgot_title, true, |a| to_step(a, Mode::Forgot))
                .at(f32::NAN, t::SPACE_16, t::SPACE_4, f32::NAN).key("forgot-at").ease(i64::from(FOLD_MS)));
            form = form.kid(el().key("remember-row").w(inner).h(t::K_SIGN_IN_REMEMBER_ROW * remembering).no_shrink().clip().opacity(remembering).anim(part(4)).kid(row().at(t::SPACE_8, t::SPACE_16, f32::NAN, f32::NAN).kid(tap(
                row().key("remember").items_center().gap(t::K_SIGN_IN_REMEMBER_GAP).kid(tick).kid(line(&sign_in.remember_title, t::T_SIGN_IN_LINE, Font::Medium, w(t::W85))),
                t::PRESS_CHIP,
                |a| a.pages.remember = !a.pages.remember,
            ))).maybe(forgot));
        }
        form = form.kid(el().key("auth-at").fill_w().margin(0., t::K_SIGN_IN_BUTTON_TOP, 0., 0.).anim(part(4))
            .kid(ds::button(app, "auth", ds::Kind::Primary, ds::Size::L, None, &sign_in.button, move |a| auth(a)).fill_w()));
        // (a code's step: Send again under the button, once a minute's gone)
        if sign_in.resend {
            form = form.kid(row().w(inner).center().margin(0., t::SPACE_16, 0., 0.)
                .kid(link(app, "resend", &sign_in.resend_title, sign_in.resend_ready, |a| a.session.send(zcore::session::Event::Resend))));
        }
        if !self.error.is_empty() {
            form = form.kid(ds::error_text(&self.error, true).fill_w().margin(0., t::SPACE_12, 0., 0.));
        }

        // (an error shakes the card: ±8, four times, 84 ms each)
        let shaking = self.shake.map(|at| at.elapsed().as_millis() as f32);
        let _ = panel;
        // (a code's step and Forgot: back, at the card's top left, to where it came from)
        let stepped = sign_in.back.is_some();
        let to = sign_in.back.unwrap_or(Mode::LogIn);
        let mut back = bare_icon_button("sign-back", "arrow_left", t::K_SIGN_IN_REVEAL, t::K_PAGE_FIELD_GLYPH, if app.kit.hover_within(k("sign-back")) { t::C_WHITE } else { w(t::W70) }, move |a| to_step(a, to)).at(t::SPACE_16, t::SPACE_16, f32::NAN, f32::NAN).opacity(if stepped { 1. } else { 0. }).ease(i64::from(FOLD_MS));
        if !stepped {
            back = back.decorative();
        }
        let card = col().key("login-card").w(card_w).radius(t::R_PANEL).clip().anim(part(0))
            .kid(el().cover().decorative().bg(FLAT_PANEL))
            .kid(col().fill_w().pad4(t::K_SIGN_IN_CARD_PAD, t::K_SIGN_IN_CARD_TOP, t::K_SIGN_IN_CARD_PAD, t::SPACE_24).kid(form))
            .kid(back);
        // the two as folder tabs on the card's top edge, past its rounded corner: the chosen one the card's own colour,
        // running down into it (one piece with it), the other darker behind; Create account first
        let tab = |key: &str, label: &str, on: bool, f: Rc<dyn Fn(&mut App)>| {
            let hot = app.kit.hover_within(k(key));
            tap(
                col().key(key).h(t::K_SIGN_IN_FOLDER_TAB).pad4(t::K_SIGN_IN_FOLDER_TAB_PAD, 0., t::K_SIGN_IN_FOLDER_TAB_PAD, t::SPACE_16).margin(0., 0., 0., -t::SPACE_16).radius(t::K_SIGN_IN_FOLDER_TAB_RADIUS).center()
                    // (the other a lighter grey that reads over the dimmed wall, lighter still under the pointer)
                    // (the chosen one white with black words, as the app's chosen chips; the other grey)
                    .bg(if on { t::C_WHITE } else if hot { t::C_TAB_OFF_HOT } else { t::C_TAB_OFF })
                    .kid(line(label, t::T_SIGN_IN_LINE, if on { Font::Bold } else { Font::Medium }, if on { t::C_BLACK } else if hot { t::C_WHITE } else { w(t::W85) })),
                t::PRESS_CHIP,
                move |a| f(a),
            )
        };
        let tabs = row().gap(t::SPACE_6).margin(t::R_PANEL - t::SPACE_4, 0., 0., 0.)
            .kid(tab("mode-up", &sign_in.create_tab_title, mode == Mode::Create, Rc::new(|a: &mut App| to_step(a, Mode::Create))))
            .kid(tab("mode-in", &sign_in.log_in_tab_title, mode != Mode::Create, Rc::new(|a: &mut App| to_step(a, Mode::LogIn))));
        // (the tabs on Create and Log in; a code's step and Forgot stand on their own)
        let steps = sign_in.tabs;
        let mut tabs = tabs.key("login-tabs").opacity(if steps { 1. } else { 0. }).ease(i64::from(FOLD_MS)).anim(part(1));
        if !steps {
            tabs = tabs.decorative();
        }
        let mut panel_ = col().key("login-panel").w(card_w).items_start().kid(tabs).kid(card);
        if let Some(ms) = shaking {
            let dx = if ms < 336. { t::SPACE_8 * (std::f32::consts::PI * ms / 84.).sin() } else { 0. };
            panel_ = panel_.margin(dx * 2., 0., 0., 0.);
        }
        col().cover().kid(wall).kid(col().cover().center().kid(panel_))
            .maybe(self.sign_in_notice(app))
    }

    /// The sign-in card's notice (an email that can't sign up, a code sent again): the update card's look, coming down
    /// from the top in the middle, and away again when the core lets it go.
    fn sign_in_notice(&self, app: &App) -> Option<El> {
        let sign_in = &app.session.view().sign_in;
        let notice = {
            let mut notice = self.notice.borrow_mut();
            if sign_in.toasts != self.toasts_seen.get() {
                self.toasts_seen.set(sign_in.toasts);
                *notice = sign_in.toast.clone().map(|words| (words, Instant::now(), None));
            }
            if let Some(n) = notice.as_mut().filter(|n| n.2.is_none() && sign_in.toast.is_none()) {
                n.2 = Some(Instant::now());
            }
            notice.clone()
        };
        let (words, at, gone) = notice.as_ref()?;
        let (coming, going) = (at.elapsed().as_millis() < 40, gone.is_some());
        let card = row().key("sign-notice").pad4(t::SPACE_14, t::SPACE_12, t::SPACE_16, t::SPACE_12).items_center().gap(t::SPACE_10)
            .bg(FLAT_PANEL).radius(t::K_SIGN_IN_NOTICE_RADIUS).border(t::S_HAIRLINE, w(t::W22))
            .kid(icon("info", t::K_SIGN_IN_NOTICE_ICON, t::C_WHITE))
            .kid(line(words, t::T_SIGN_IN_LINE, Font::Medium, t::C_WHITE));
        let top = if coming || going { -t::K_SIGN_IN_NOTICE_AWAY } else { t::K_SIGN_IN_NOTICE_TOP };
        Some(row().key("sign-notice-row").fill_w().center().at(0., top, 0., f32::NAN).decorative().kid(card).slide(460).opacity(if coming || going { 0. } else { 1. }).ease(320))
    }
}

/// A sign-in field's key in the card.
fn key_of(f: zcore::session::Field) -> &'static str {
    use zcore::session::Field as F;
    match f {
        F::Email => "email",
        F::Code => "code",
        F::Username => "username",
        F::Password => "password",
    }
}

/// On to another sign-in step: the focus to its first field, as the core lists them.
fn to_step(a: &mut App, mode: zcore::session::Mode) {
    a.session.send(zcore::session::Event::Mode { mode });
    a.kit.focus = k(a.session.view().sign_in.fields.first().map_or("username", |f| key_of(*f)));
}
/// How long a sign-in field takes to fold open or shut.
pub(crate) const FOLD_MS: u32 = 280;

impl Pages {
    /// Whether a part of the sign-in card is still folding: frames are drawn one after another until it rests.
    pub(crate) fn folding(&self) -> bool {
        self.folds.borrow().values().any(|(from, to, at)| from != to && at.elapsed().as_millis() <= u128::from(FOLD_MS))
    }

    /// How open a part of the sign-in card is now (0 shut, 1 open), easing towards `open` over FOLD_MS whenever that
    /// changes; it starts where it's first asked to be.
    fn fold(&self, key: &'static str, open: bool) -> f32 {
        let to = if open { 1. } else { 0. };
        let mut folds = self.folds.borrow_mut();
        let now = Instant::now();
        let at = |(from, to, since): (f32, f32, Instant)| {
            let p = (since.elapsed().as_millis() as f32 / FOLD_MS as f32).clamp(0., 1.);
            let eased = 1. - (1. - p).powi(3);
            from + (to - from) * eased
        };
        let entry = *folds.entry(key).or_insert((to, to, now));
        if entry.1 != to {
            folds.insert(key, (at(entry), to, now));
        }
        at(folds[key])
    }
}

/// A line of words to press (Forgot password?, Back, Send again): brighter under the pointer; dim and still when it
/// can't be pressed yet.
fn link(app: &App, key: &str, words: &str, on: bool, f: impl Fn(&mut App) + 'static) -> El {
    let hot = on && app.kit.hover_within(k(key));
    let words = line(words, t::T_SIGN_IN_LINE, Font::Medium, if !on { w(t::W30) } else if hot { t::C_WHITE } else { w(t::W85) });
    if !on {
        return el().key(key).kid(words);
    }
    tap(el().key(key).kid(words), t::PRESS_CHIP, f)
}

/// Anything that follows the scroll (the glow easing off, a title's picture dimming) glides as long as the scroll
/// itself does (kit's wheel: 140 ms), every copy alike, so they move with the page and with each other.
pub(crate) const SCROLL_EASE: i64 = 140;

/// How long a page takes to slide in or out, and the page under it to move with it (the same both ways).
const PAGE_SLIDE: i64 = 420;


/// The name at the rail's top, its version tiny and up by it (as the phone has it); Who's watching puts it in the very
/// same place.
/// Where the name sits from the window's top: under the window's three buttons.
pub(super) const BRAND_TOP: f32 = t::SPACE_4;   // (from the top of the rail, which starts under the window's strip)

/// "zf", the rail's mark while it's just icons: centred over them, the f in the page's colour.
fn zf_mark(flix: u32, width: f32) -> El {
    let size = t::T_SIDEBAR_MARK.0;
    let spec = (size, 0., -size * t::K_WORDMARK_SPACING_SHARE);
    let close = 1. + size * t::K_WORDMARK_SPACING_SHARE;
    col().w(width).h(t::K_SIDEBAR_MARK).center()
        .kid(row().items_center()
            .kid(line("z", spec, Font::Bold, t::C_INK).margin(0., 0., -close, 0.))
            .kid(line("f", spec, Font::Bold, t::C_WHITE).key("brand-f").tint(flix)))
}

fn brand_mark(flix: u32, before: Option<u32>) -> El {
    // ("flix" in the page's colour: a new colour fades in over the last as it fades out, never a jump between them)
    let size = t::T_SIDEBAR_MARK.0;
    let spec = (size, 0., -size * t::K_WORDMARK_SPACING_SHARE);
    let close = 1. + size * t::K_WORDMARK_SPACING_SHARE;
    // ("flix" drawn once, white, as a stencil over a colour the compositor glides from one to the next: the way a
    // colour change on type is done, never two copies crossfading)
    let _ = before;
    let flix_box = line("flix", spec, Font::Bold, t::C_WHITE).key("brand-flix").tint(flix);
    row().h(t::K_SIDEBAR_MARK).items_center().kid(line("zun", spec, Font::Bold, t::C_INK).margin(0., 0., -close, 0.)).kid(flix_box)
}

/// How long "zunflix" takes to become "zf" as Who's watching gives way to Home: the splash's opening, backwards.
pub(super) const MORPH_MS: f32 = 680.;

/// Where Who's watching has the name (its z).
const BRAND_X: f32 = t::K_SIGN_IN_WHO_MARK_X;

/// "zunflix" becoming the rail's "zf" as Who's watching gives way to Home: the z and the f gliding from where Who's
/// watching had the name to the rail's mark, "un" and "lix" fading as they go.
/// Laid out once where it ends (on whole pixels) and slid there by the compositor, as the splash is: laying it out
/// again every frame snapped the letters to pixels and made them jitter.
fn brand_morph(app: &App, flix: u32) -> El {
    let size = t::T_SIDEBAR_MARK.0;
    let spec = (size, 0., -size * t::K_WORDMARK_SPACING_SHARE);
    let close = 1. + size * t::K_WORDMARK_SPACING_SHARE;
    let style = ts(spec, Font::Bold, t::C_INK, 1);
    let adv = |s: &str| app.kit.g.measure(s, &style, f32::INFINITY).0 - close;
    let scale = crate::pages::scale().max(1.);
    let snap = |v: f32| (v * scale).round() / scale;
    // (the rail's zf, centred over its icons, as zf_mark lays it)
    let (wz, wf, wun) = (adv("z"), adv("f"), adv("un"));
    let to = snap(RAIL_PAD + (RAIL - 2. * RAIL_PAD - (wz + app.kit.g.measure("f", &style, f32::INFINITY).0)) / 2.);
    let (z_end, f_end) = (to, snap(to + wz));
    let (z_from, f_from) = (BRAND_X, BRAND_X + wz + wun);
    let ms = MORPH_MS as i64;
    let top = crate::WINDOW_STRIP + BRAND_TOP;
    let room = size * t::K_SPLASH_OVERHANG_SHARE;   // (a bold letter's overhang, inside a folding piece's clip)
    // (the splash's opening played backwards: "un" and "lix" close up behind the z and the f as those slide together)
    let piece = |key: &str, s: &str, colour: u32, x: f32, dx: f32, fold: bool| {
        let word = line(s, spec, Font::Bold, colour).no_shrink();
        let at = row().key(key).at(x, top, f32::NAN, f32::NAN).h(t::K_SIDEBAR_MARK).items_center().anim(Anim::Close(dx, ms, fold));
        if fold { at.w(adv(s) + room).clip() } else { at }.kid(word)
    };
    el().key("brand-morph").cover().decorative()
        .kid(piece("brand-un", "un", t::C_INK, f_end, z_from + wz - f_end, true))
        .kid(piece("brand-lix", "lix", flix, snap(f_end + wf), f_from - f_end, true))
        .kid(piece("brand-z", "z", t::C_INK, z_end, z_from - z_end, false))
        .kid(piece("brand-f", "f", flix, f_end, f_from - f_end, false))
}

/// The loading screen, the website's and the phone's, move for move (zsplash.js): "z" slides in, the grey "f" drops
/// onto it with a springy bounce, then the name opens out in place: "un" grows between them and "lix" after the f.
/// While the app gets ready a band of light passes over the name (again every 0.4 s). `t`: ms since it began
/// (infinite: still, opened out: as it eases away). Drawn afresh every frame while it shows.
pub(crate) fn splash(app: &App, t: f32) -> El {
    col().key("splash").cover().center().bg(t::C_BG).kid(splash_word(app, t, if app.kit.width >= t::K_SPLASH_LARGE_FROM { t::T_SPLASH_LARGE.0 } else { t::T_SPLASH.0 }))
}

/// The splash's name alone (`size` its type), as wide as the window: the loading screen's, and the bootstrapper's.
pub(crate) fn splash_word(app: &App, t: f32, size: f32) -> El {
    let spacing = -size * t::K_SPLASH_SPACING_SHARE;
    let style = |color: u32| TextStyle { spacing, line: size * t::K_SPLASH_LINE_RATIO, ..st(Font::Bold, size, color, 1) };
    // (each piece's own advance: the measure gives back its last tight gap and a pixel, taken off so they meet)
    let adv = |s: &str| app.kit.g.measure(s, &style(t::C_INK), f32::INFINITY).0 - 1. - spacing.abs();
    let (wz, wun, wf, wlix) = (adv("z"), adv("un"), adv("f"), adv("lix"));
    let lh = (size * t::K_SPLASH_LINE_RATIO).ceil();
    let (ink, grey) = (t::C_INK, crate::theme::flix());
    // laid out once, where the name ends up (on whole pixels); the compositor plays the rest, as a browser plays the
    // website's: moving type drawn once and slid smoothly, never laid out again and snapped every frame (that made the
    // letters twitch: the known cause, research says, is pixel snapping during animation)
    let still = !t.is_finite();
    let total = wz + wun + wf + wlix;
    let scale = crate::pages::scale().max(1.);
    let snap = |v: f32| (v * scale).round() / scale;
    let span = app.kit.width;
    let start = snap((span - total) / 2.);
    let (z_x, un_x, f_x, lix_x) = (start, snap(start + wz), snap(start + wz + wun), snap(start + wz + wun + wf));
    let room = size * t::K_SPLASH_OVERHANG_SHARE;   // (a bold letter's overhang, inside the pieces shown from their left)
    // (how far the z and the f sit from here while the name's closed: the z with the f against it, in the middle)
    let z_shift = (wun + wlix) / 2.;
    let f_shift = (wlix - wun) / 2.;
    let anim = |piece: u8, a: f32| if still { Anim::None } else { Anim::Splash(piece, a) };
    let piece = |s: &str, color: u32| text(s, style(color)).no_shrink();
    // (built afresh for each window size: moving a letter while it plays stops it, and the window maximises just as
    // the splash begins; a new size starts the whole sequence again where it belongs)
    let mut word = el().key(&format!("splash-word-{}x{}", span.round() as i32, app.kit.height.round() as i32)).size(span, lh)
        .kid(el().key("splash-un").at(un_x, 0., f32::NAN, f32::NAN).size(f_x - un_x + room, lh).clip().anim(anim(1, z_shift)).kid(piece("un", ink)))
        .kid(el().key("splash-lix").at(lix_x, 0., f32::NAN, f32::NAN).size(wlix + room, lh).clip().anim(anim(3, f_shift)).kid(piece("lix", grey)))
        .kid(el().key("splash-z").at(z_x, 0., f32::NAN, f32::NAN).anim(anim(0, z_shift)).kid(piece("z", ink)))
        .kid(el().key("splash-f").at(f_x, 0., f32::NAN, f32::NAN).anim(anim(2, f_shift)).kid(piece("f", grey)));
    if !still {
        // (the band of light, on the letters only: thin slices of a white copy of the name, brightest in the middle)
        let profile = [0.1, 0.3, 0.6, 0.9, 0.9, 0.6, 0.3, 0.1];
        for (i, a) in profile.iter().enumerate() {
            word = word.kid(el().key(&format!("splash-shine-{i}")).at(start, 0., f32::NAN, f32::NAN).size(total + room, lh).clip().opacity(*a)
                .anim(Anim::Shine(i as f32, profile.len() as f32))
                .kid(el().at(0., 0., f32::NAN, f32::NAN).kid(piece("z", t::C_WHITE)))
                .kid(el().at(un_x - start, 0., f32::NAN, f32::NAN).kid(piece("un", t::C_WHITE)))
                .kid(el().at(f_x - start, 0., f32::NAN, f32::NAN).kid(piece("f", t::C_WHITE)))
                .kid(el().at(lix_x - start, 0., f32::NAN, f32::NAN).kid(piece("lix", t::C_WHITE))));
        }
    }
    word.decorative()
}

/// Where every page's content starts: below its head (ds::HEAD).
pub(super) const HEAD: f32 = ds::HEAD;

/// A pushed page's head (the title page, search, Settings, Downloads, Watch history): ds::head with Back, in the
/// page's gutter.
/// A page's scroller, under the window's strip.
pub(super) fn scroller(key: &str) -> El {
    col().key(key).cover().scroll_y()
}

pub(super) fn page_head(app: &App, key: &str, name: Option<&str>, scrolled: bool, trailing: Option<El>) -> El {
    ds::head(app, key, gutter(view_w(app)), Some(Rc::new(|a: &mut App| Pages::go_back(a))), name, scrolled, trailing)
}

/// The space at a page's sides: the phone's 16, more as the window widens.
pub(super) fn gutter(width: f32) -> f32 {
    (width * t::K_SHELL_GUTTER_SHARE).clamp(t::S_GUTTER, t::SPACE_48)
}
