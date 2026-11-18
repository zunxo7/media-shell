//! Manhwa, from the rail: the shelf (chips, a search in the head, the covers as a grid), a series (its cover, what it's
//! about, its chapters, each with its download), and the reader (the chapter's pages as one strip in a readable column,
//! zoomed as a browser zooms, the chapter's end with the next one). Each is the core's screen (`zcore::manhwa`): this
//! draws its view, tells it what happened, and loads the pages the reader asks for.
use super::home::{grid_columns, grid_skeleton, search_box, virtual_rows, CHIP_ROOM, SWAP_MS};
use super::shell::{gutter, page_head, scroller};
use super::*;
use crate::core_screen::CoreScreen;
use crate::downloads::{self, State};
use crate::manhwa::{Chapter, Comic};
use zcore::manhwa::reader::{self, Reader};
use zcore::manhwa::series::{self, Series};
use zcore::manhwa::shelf::{self, Card, Shelf};
use zcore::manhwa::Strip;

/// What `finding` and the search field are named for the shelf.
const PAGE: &str = "manhwa";
/// The shelf's scroller.
const SHELF: &str = "page-manhwa";
/// The tallest piece of a page drawn as one picture: a merged chapter's page can be 20,000 px tall, past the largest
/// texture the GPU takes (16,384), and far more than a screen needs held at once.
const SLICE: f32 = 1200.;
/// A notch of the wheel (Windows' WHEEL_DELTA): Ctrl and the wheel zoom a step a notch.
const NOTCH: f32 = 120.;
/// The arrow keys step as far as a notch of the wheel does (Windows' WHEEL_DELTA, read as points by the kit).
const ARROW_STEP: f32 = 120.;
/// How far the shelf scrolls before the core hears of it again (it only wants to know when the end comes near).
const SHELF_TELL: f32 = 48.;

/// The shelf, the series opened and the chapter being read, as the core has them; and what's been told of each.
pub(super) struct Manhwa {
    shelf: CoreScreen<Shelf>,
    series: CoreScreen<Series>,
    reader: CoreScreen<Reader>,
    /// The series open, and the one the chapter being read is of.
    comic: Option<Comic>,
    reading: Option<Comic>,
    /// What the search field had when last told.
    typed: String,
    /// What the shelf, the series and the reader were last told of this window, so each hears only of a change.
    told_end: Option<f32>,
    told_saved: Option<Vec<String>>,
    told_layout: Option<(f32, f32, f32, f32)>,
    told_pos: Option<f32>,
    went: u32,
    /// The reader's pages on their way: (round, index).
    started: HashSet<(u32, u32)>,
}

impl Default for Manhwa {
    fn default() -> Self {
        crate::manhwa::move_reading();
        Self {
            shelf: CoreScreen::new(|a| &mut a.pages.manhwa.shelf, |_, _| {}),
            series: CoreScreen::new(|a| &mut a.pages.manhwa.series, |_, _| {}),
            reader: CoreScreen::new(|a| &mut a.pages.manhwa.reader, |_, _| {}),
            comic: None,
            reading: None,
            typed: String::new(),
            told_end: None,
            told_saved: None,
            told_layout: None,
            told_pos: None,
            went: 0,
            started: HashSet::new(),
        }
    }
}

fn series_key(path: &str) -> String {
    format!("page-series-{path}")
}

fn reader_key(path: &str) -> String {
    format!("page-reader-{path}")
}

/// The reader's readable column (what 100% is) and the room the window has for its pages (all a zoom can widen them to):
/// the page's width less its gutters.
fn column_room(window: f32) -> (f32, f32) {
    let page = (window - STAGE_X).max(t::K_MANHWA_PAGE_MIN);
    let room = page - gutter(page) * 2.;
    (room.min(t::K_MANHWA_STRIP), room)
}

impl Pages {
    // ---------- the shelf ----------
    /// The rail's Manhwa: the shelf, opened fresh from wherever you are, as the rail's other pages are.
    pub(super) fn open_manhwa(&mut self) {
        self.clear_for();
        self.push(Route::Manhwa);
        self.manhwa.shelf.send(shelf::Event::Opened);
    }

    /// Typed in the shelf's search: told to the core, which asks once two letters are in and typing pauses.
    fn manhwa_typed(app: &mut App) {
        let query = app.kit.text_of(&super::find_field(PAGE));
        let m = &mut app.pages.manhwa;
        if m.typed == query {
            return;
        }
        m.typed = query.clone();
        m.shelf.send(shelf::Event::Typed { text: query });
        app.kit.reset_scroll(k(&format!("{}-page", super::find_field(PAGE))));
    }

    /// The shelf's search opening: nothing typed yet.
    pub(super) fn manhwa_search_reset(&mut self) {
        self.manhwa.typed.clear();
        self.manhwa.shelf.send(shelf::Event::Search { open: true });
    }

    /// The shelf's search closing.
    pub(super) fn manhwa_search_closed(&mut self) {
        self.manhwa.typed.clear();
        self.manhwa.shelf.send(shelf::Event::Search { open: false });
    }

    /// The shelf: its name with search in the head, the chips, the covers as a grid (more as its end comes near);
    /// searching, what Asura has by the name typed.
    pub(super) fn manhwa_view(&self, app: &App) -> El {
        let (width, height) = (view_w(app), app.kit.height);
        let gutter = gutter(width);
        let v = self.manhwa.shelf.view();
        let pos = app.kit.scroll_of(k(SHELF)).pos;
        // (search right here, as the sections': the name gives way to Back and the field; until two letters are
        // typed the shelf stays, its chips folding away, then what Asura has by that name)
        let finding = self.finding == Some(PAGE);
        let found_key = format!("{}-page", super::find_field(PAGE));
        let found_pos = app.kit.scroll_of(k(&found_key)).pos;
        let typed = finding && v.search.shown;
        let field = |p: &Pages, app: &App, w_: f32| p.manhwa_field(app, w_);
        let head = ds::head(app, "manhwa-head", gutter, None, None, if typed { found_pos > 4. } else { pos > 4. },
            Some(self.finding_head(app, PAGE, &v.title, finding, width - gutter * 2., field, None)));
        let failed = v.trouble.is_some();
        let mut body = col().fill_w().no_shrink().pad4(0., ds::HEAD, 0., 0.);
        let mut chips_over = None;
        if !failed {
            let chips = v.chips.iter().enumerate().map(|(i, label)| {
                ds::chip(app, &format!("manhwa-chip-{i}"), label, i as u32 == v.chip, move |a| {
                    a.kit.reset_scroll(k(SHELF));
                    a.pages.manhwa.shelf.send(shelf::Event::Chip { index: i as u32 });
                })
            }).collect();
            let mut chip_line = chip_row("manhwa-chips", chips, gutter).at(0., ds::HEAD, 0., f32::NAN).opacity(if finding { 0. } else { 1. }).ease(SWAP_MS);
            if finding { chip_line = chip_line.decorative(); }
            chips_over = Some(chip_line);
            body = body.kid(el().key("manhwa-chip-room").h(if finding { 0. } else { CHIP_ROOM }).no_shrink());
        }
        let grid_top = ds::HEAD + if finding || failed { 0. } else { CHIP_ROOM };
        if v.loading {
            body = body.kid(grid_skeleton(width - gutter * 2., width, 6).margin(gutter, 0., gutter, 0.));
        } else {
            body = body.kid(comic_grid("manhwa-grid", &v.cards, width - gutter * 2., width, (height, pos - grid_top)).margin(gutter, 0., gutter, 0.).slide(SWAP_MS));
        }
        if v.more {
            body = body.kid(col().fill_w().h(t::K_MANHWA_MORE).no_shrink().center().kid(spinner(t::K_MANHWA_SPINNER, w(t::W60))));
        }
        body = body.kid(el().h(t::SPACE_40).no_shrink());
        let mut shelf = el().key("manhwa-shelf").cover().opacity(if typed { 0. } else { 1. }).ease(SWAP_MS)
            .kid(scroller(SHELF).kid(body.maybe(chips_over)));
        if let Some(trouble_) = &v.trouble {
            shelf = shelf.kid(el().cover().pad4(0., ds::HEAD, 0., 0.)
                .kid(trouble("manhwa-trouble", &trouble_.message, trouble_.offline, trouble_.busy, |a| a.pages.manhwa.shelf.send(shelf::Event::Retry), None)));
        }
        if typed { shelf = shelf.decorative(); }
        let mut found = el().key("manhwa-found").cover().opacity(if typed { 1. } else { 0. }).ease(SWAP_MS)
            .kid(scroller(&found_key).kid(self.manhwa_found(app, &v.search, found_pos)));
        if !typed { found = found.decorative(); }
        el().cover().kid(shelf).kid(found).kid(head)
    }

    /// What the search found: how many over the covers, or "Nothing found".
    fn manhwa_found(&self, app: &App, search: &shelf::Search, pos: f32) -> El {
        let (width, height) = (view_w(app), app.kit.height);
        let gutter = gutter(width);
        let results = col().key("manhwa-results").fill_w().no_shrink().pad4(gutter, ds::HEAD, gutter, t::SPACE_40)
            .kid(line(&search.note, t::T_ROW_TITLE, Font::Bold, t::C_WHITE).margin(0., t::SPACE_2, 0., t::SPACE_14));
        if let Some(nothing) = &search.empty {
            results.kid(empty("search", &nothing.title, &nothing.line))
        } else if search.busy {
            results.kid(grid_skeleton(width - gutter * 2., width, 6))
        } else {
            results.kid(comic_grid("manhwa-found", &search.cards, width - gutter * 2., width, (height, pos - ds::HEAD - t::SPACE_40)))
        }
    }

    /// The shelf's search field, as the sections'.
    fn manhwa_field(&self, app: &App, field_w: f32) -> El {
        let v = self.manhwa.shelf.view();
        search_box(app, &super::find_field(PAGE), &v.search.hint, field_w, Rc::new(Self::manhwa_typed), v.search.busy, !self.manhwa.typed.is_empty())
    }

    // ---------- a series ----------
    fn open_series(&mut self, comic: Comic) {
        self.push(Route::Series(comic.path.clone()));
        self.manhwa.told_saved = None;
        self.manhwa.series.send(series::Event::Opened { comic: comic.clone() });
        self.manhwa.comic = Some(comic);
    }

    /// A series: its cover beside its name, facts and Read (or Continue, from the chapter you're up to), what it's
    /// about (four lines; a click for the rest), then its chapters as rows.
    pub(super) fn series_view(&self, app: &App, path: &str) -> El {
        let (width, height) = (view_w(app), app.kit.height);
        let gutter = gutter(width);
        let col_w = (width - gutter * 2.).min(t::K_TITLE_COLUMN);
        let v = self.manhwa.series.view();
        let comic = self.manhwa.comic.clone().unwrap_or_else(|| Comic { path: path.to_string(), ..Default::default() });
        let key = series_key(path);
        let pos = app.kit.scroll_of(k(&key)).pos;
        let _ = self.glow_of(&format!("series-{path}"), v.glow.clone().map_or(0, |cover| glow_colour(None, Some(cover))));
        let head = page_head(app, "series-head", (pos > 4.).then_some(v.title.as_str()), pos > 4., None);
        if let Some(trouble_) = &v.trouble {
            return el().cover()
                .kid(trouble("series-trouble", &trouble_.message, trouble_.offline, trouble_.busy, |a| a.pages.manhwa.series.send(series::Event::Retry), None))
                .kid(head);
        }
        let chapters: Vec<Chapter> = v.rows.iter().map(|r| r.chapter.clone()).collect();

        // ---- the cover, and beside it the name, facts and Read
        let cover_w = t::K_MANHWA_WIDE_COVER;
        let cover_h = cover_w * t::S_POSTER_RATIO;
        let cover = picture(el().size(cover_w, cover_h).no_shrink(), &v.cover, t::R_POSTER_CARD).bg(t::C_CARD);
        let read: El = match &v.read {
            None => skeleton(el().w(t::K_TITLE_PLAY_SKELETON_W).h(ds::Size::L.h()), ds::Size::L.h() / 2.),
            Some(read) => {
                let (series_, chapter) = (comic.clone(), read.chapter.clone());
                ds::button(app, "series-read", ds::Kind::Primary, ds::Size::L, Some("book_open"), &read.label,
                    move |a| a.pages.read_chapter(&series_, chapter.clone()))
            }
        };
        let buttons = row().gap(t::SPACE_12).items_center().margin(0., t::SPACE_24, 0., 0.).kid(read)
            .maybe((!chapters.is_empty()).then(|| download_all(app, &comic, &chapters)));
        let words_ = col().grow().margin(t::SPACE_28, 0., 0., 0.).items_start()
            .kid(text(&v.title, ts(t::T_BIG_TITLE, Font::Bold, t::C_WHITE, 3)).fill_w())
            .kid(line(&v.facts, t::T_TITLE_FACTS, Font::Medium, w(t::W85)).margin(0., t::SPACE_12, 0., 0.))
            .kid(buttons);
        let top = row().w(col_w).items_end().kid(cover).kid(words_);
        let mut body = col().fill_w().no_shrink().items_center().pad4(0., ds::HEAD, 0., 0.).kid(top);
        if let Some(about) = &v.about {
            body = body.kid(tap(
                el().key("series-story").w(col_w).margin(0., t::SPACE_24, 0., 0.)
                    .kid(text(about, ts(t::T_STORY, Font::Sans, w(t::W85), if v.about_open { 0 } else { 4 })).fill_w()),
                1.,
                |a| a.pages.manhwa.series.send(series::Event::About),
            ));
        }
        body = body.kid(line(&v.heading, t::T_TITLE_SECTION, Font::Bold, t::C_WHITE).w(col_w).margin(0., t::SPACE_40, 0., t::SPACE_10));

        // ---- the chapters: only those near the screen built
        let row_h = t::S_STILL_H + t::SPACE_12 * 2.;
        let mut list = col().key(&format!("series-chapters-{path}")).w(col_w);
        if v.rows.is_empty() {
            for _ in 0..6 {
                list = list.kid(row().w(col_w).h(row_h).pad_xy(t::SPACE_12, 0.).items_center()
                    .kid(skeleton(el().size(t::S_STILL_W, t::S_STILL_H).no_shrink(), t::K_TITLE_STILL_RADIUS))
                    .kid(col().grow().margin(t::SPACE_16, 0., 0., 0.)
                        .kid(skeleton(el().pct_w(0.4).h(t::K_MANHWA_NAME_SKELETON), t::K_MANHWA_NAME_SKELETON / 2.))
                        .kid(skeleton(el().pct_w(0.6).h(t::K_MANHWA_LINE_SKELETON).margin(0., t::SPACE_8, 0., 0.), t::K_MANHWA_LINE_SKELETON / 2.))));
            }
        } else {
            // (where the list starts down the page, near enough: the build reaches a screen past either edge)
            let list_top = ds::HEAD + cover_h + t::SPACE_24 + t::T_STORY.1 * 4. + t::SPACE_40 + t::SPACE_40;
            list = list.kids(virtual_rows(v.rows.len(), row_h, height, pos - list_top, |i| chapter_row(app, &comic, &v.rows[i], col_w, row_h)));
        }
        body = body.kid(list).kid(el().h(t::SPACE_64).no_shrink());
        el().cover().kid(scroller(&key).kid(body)).kid(head)
    }

    /// A chapter downloaded, opened from Downloads (`row`: its download): read from its files.
    pub(super) fn read_saved(&mut self, row: &serde_json::Value) {
        let text = |k: &str| row[k].as_str().unwrap_or_default().to_string();
        let series_ = Comic { path: text("titleId"), title: text("title"), cover: text("art"), thumb: text("cover"), rating: String::new() };
        self.read_chapter(&series_, Chapter { path: text("key"), label: text("label"), thumb: text("thumb"), ..Default::default() });
    }

    // ---------- the reader ----------
    /// A chapter, read from where it was left: in place of the one being read (its next or previous), else over the
    /// series.
    fn read_chapter(&mut self, series_: &Comic, chapter: Chapter) {
        if matches!(self.route(), Some(Route::Reader(_))) {
            self.nav.send(zcore::nav::Event::Back);
        }
        let saved = downloads::chapter_pages(&chapter.path).map_or(0, |pages| pages.len() as u32);
        self.push(Route::Reader(chapter.path.clone()));
        let m = &mut self.manhwa;
        m.told_pos = None;
        m.reading = Some(series_.clone());
        m.reader.send(reader::Event::Opened { series: series_.clone(), chapter, saved });
    }

    /// Just laid out: the reader scrolled where the core asks (where the chapter was left, once there's that much of
    /// it; or where a zoom keeps what was being read), in the same frame.
    pub(super) fn place_reader(&mut self, kit: &mut crate::kit::Kit) {
        let Some(Route::Reader(path)) = self.stack.last() else { return };
        let v = self.manhwa.reader.view();
        let Some(go) = v.go_to.filter(|g| v.open && g.seq != self.manhwa.went) else { return };
        let key = k(&reader_key(path));
        if go.pos > 0.5 && kit.scroll_of(key).max + 0.5 < go.pos && v.end.is_none() {
            return;
        }
        kit.scroll_now(key, go.pos);
        self.manhwa.went = go.seq;
        self.manhwa.told_pos = Some(go.pos);
        self.manhwa.reader.send(reader::Event::Went { seq: go.seq });
    }

    /// Zoomed a step in or out (`inward` None: back to 100%), keeping the strip's point `at` points down the window
    /// where it is.
    fn zoom_reader(app: &mut App, inward: Option<bool>, at: f32) {
        if !matches!(app.pages.route(), Some(Route::Reader(_))) {
            return;
        }
        // (the strip starts under the head: a point `at` down the window is that much less HEAD down the strip)
        app.pages.manhwa.reader.send(reader::Event::Zoom { inward, at: at - ds::HEAD });
    }

    /// Ctrl and the wheel over the reader: a step in or out a notch, about the pointer (`y`). False when the reader
    /// isn't showing.
    pub fn reader_zoom_wheel(app: &mut App, delta: f32, y: f32) -> bool {
        if app.player.is_some() || !matches!(app.pages.route(), Some(Route::Reader(_))) {
            return false;
        }
        let notches = (delta / NOTCH).round() as i32;
        for _ in 0..notches.unsigned_abs().max(1) {
            Self::zoom_reader(app, Some(delta > 0.), y);
        }
        true
    }

    /// Time passing on Manhwa's pages: the shelf hears how near its end is, a series which of its chapters are
    /// downloaded, the reader the window's room and where it's scrolled; the pages the reader asks for are loaded.
    pub(super) fn manhwa_tick(&mut self, kit: &mut crate::kit::Kit) {
        let route = self.route().cloned();
        let m = &mut self.manhwa;
        if matches!(route, Some(Route::Manhwa)) && self.finding.is_none() {
            let scroll = kit.scroll_of(k(SHELF));
            let to_end = scroll.max - scroll.pos;
            if m.told_end.is_none_or(|told| (told - to_end).abs() >= SHELF_TELL) {
                m.told_end = Some(to_end);
                m.shelf.send(shelf::Event::Scrolled { to_end, screen: kit.height });
            }
        }
        if matches!(route, Some(Route::Series(_))) {
            let saved: Vec<String> = m.series.view().rows.iter()
                .filter(|r| downloads::progress_of(&r.chapter.path).is_some_and(|(state, _)| state == State::Done))
                .map(|r| r.chapter.path.clone()).collect();
            if m.told_saved.as_ref() != Some(&saved) {
                m.told_saved = Some(saved.clone());
                m.series.send(series::Event::Downloaded { paths: saved });
            }
        }
        if !self.stack.iter().any(|r| matches!(r, Route::Reader(_))) {
            if m.reading.take().is_some() {
                m.reader.send(reader::Event::Left);
            }
            return;
        }
        let Some(Route::Reader(path)) = route else { return };
        let (column, room) = column_room(kit.width);
        let layout = (column, room, kit.height, kit.g.scale);
        if m.told_layout != Some(layout) {
            m.told_layout = Some(layout);
            m.reader.send(reader::Event::Layout { column, room, screen: kit.height, scale: kit.g.scale });
        }
        let pos = kit.scroll_of(k(&reader_key(&path))).pos;
        if m.told_pos != Some(pos) {
            m.told_pos = Some(pos);
            m.reader.send(reader::Event::Scrolled { pos });
        }
        let v = m.reader.view();
        for load in &v.load {
            if !m.started.insert((v.round, load.index)) {
                continue;
            }
            let address = match &load.url {
                Some(url) => url.clone(),
                None => format!("file:{}", downloads::chapter_dir(&path).join(&load.file).display()),
            };
            let (round, index) = (v.round, load.index);
            spawn(async move {
                let size = crate::images::page_size(&address).await;
                post(move |a| {
                    let event = match size {
                        Some((width, height)) => reader::Event::Measured { round, index, width, height },
                        None => reader::Event::Missed { round, index, status: 0, offline: a.pages.offline },
                    };
                    a.pages.manhwa.reader.send(event);
                });
            });
        }
    }

    /// The reader's keys: the arrows, Page Up and Down, Space, Home and End scroll; left and right go a chapter back or
    /// on; Ctrl and + or - zoom (the main keys' or the number pad's), Ctrl+0 back to 100%, as a browser's. False when
    /// the reader isn't what's showing.
    pub fn reader_keydown(app: &mut App, vk: u16) -> bool {
        use crate::platform::key;
        if app.player.is_some() || app.pages.sheet.is_some() || app.pages.menu {
            return false;
        }
        let Some(Route::Reader(path)) = app.pages.route().cloned() else { return false };
        if crate::platform::ctrl_down() {
            let zoom = match vk {
                key::PLUS | key::PAD_PLUS => Some(Some(true)),
                key::MINUS | key::PAD_MINUS => Some(Some(false)),
                key::ZERO | key::PAD_ZERO => Some(None),
                _ => None,
            };
            if let Some(zoom) = zoom {
                let middle = app.kit.height / 2.;
                Self::zoom_reader(app, zoom, middle);
                return true;
            }
        }
        if vk == key::LEFT || vk == key::RIGHT {
            let v = app.pages.manhwa.reader.view();
            let to = if vk == key::LEFT { v.before.clone() } else { v.after.clone() };
            if let (Some(chapter), Some(series_)) = (to, app.pages.manhwa.reading.clone()) {
                app.pages.read_chapter(&series_, chapter);
            }
            return true;
        }
        let key = k(&reader_key(&path));
        let state = app.kit.scroll_of(key);
        let screen = (app.kit.height - crate::WINDOW_STRIP - ds::HEAD) * 0.9;
        let to = match vk {
            key::DOWN => state.pos + ARROW_STEP,
            key::UP => state.pos - ARROW_STEP,
            key::PAGE_DOWN | key::SPACE => state.pos + screen,
            key::PAGE_UP => state.pos - screen,
            key::HOME => 0.,
            key::END => state.max,
            _ => return false,
        };
        let _ = app.kit.glide(key, to);
        true
    }

    /// A chapter, top to bottom, in a readable column: the bar (Back, the series and chapter, the page you're on, the
    /// chapter before and after), the pages (a spinner and how far while it opens), its end.
    pub(super) fn reader_view(&self, app: &App, path: &str) -> El {
        let width = view_w(app);
        let gutter = gutter(width);
        let key = reader_key(path);
        let pos = app.kit.scroll_of(k(&key)).pos;
        let v = self.manhwa.reader.view();
        if self.manhwa.reading.is_none() {
            return el().cover().kid(page_head(app, "reader-head", None, false, None));
        }
        let mut body = col().fill_w().no_shrink().items_center().pad4(0., ds::HEAD, 0., 0.);
        if v.open {
            body = body.kid(strip_pieces(&key, path, v, app.kit.g.scale, pos, app.kit.height));
            if let Some(end) = &v.end {
                body = body.kid(self.chapter_end(app, end, v));
            } else {
                // the rest hasn't come: a spinner where it stops while it's coming, else why, with Try again
                let rest = col().fill_w().h(t::K_MANHWA_REST).no_shrink().center();
                body = body.kid(match &v.stopped {
                    None => rest.kid(spinner(t::K_MANHWA_SPINNER, w(t::W60))),
                    Some(stopped) => rest.kid(trouble("reader-rest", &stopped.message, stopped.offline, stopped.busy, |a| a.pages.manhwa.reader.send(reader::Event::Retry), None)),
                });
            }
        }
        let mut page = el().cover().kid(scroller(&key).kid(body));
        if !v.open {
            // opening: the spinner and how far, or why it couldn't
            page = page.kid(match &v.trouble {
                Some(trouble_) => trouble("reader-trouble", &trouble_.message, trouble_.offline, trouble_.busy, |a| a.pages.manhwa.reader.send(reader::Event::Retry), None),
                None => col().cover().center()
                    .kid(spinner(t::K_MANHWA_OPEN_SPINNER, w(t::W85)))
                    .kid(line(&v.chapter, t::T_PHONE_ROW, Font::Bold, t::C_WHITE).margin(0., t::SPACE_16, 0., 0.))
                    .maybe(v.progress.as_ref().map(|f| line(f, t::T_NOTE, Font::Sans, w(t::W60)).margin(0., t::SPACE_4, 0., 0.))),
            });
        }
        page.kid(self.reader_bar(app, v, pos, gutter))
    }

    /// The reader's bar: Back, the series and chapter, then the page you're on and the chapter before and after.
    fn reader_bar(&self, app: &App, v: &reader::View, pos: f32, gutter: f32) -> El {
        let series_ = self.manhwa.reading.clone();
        let step = |key: &str, glyph: &'static str, to: Option<Chapter>| {
            let can = to.is_some();
            let target = to.zip(series_.clone());
            ds::icon_button(app, key, ds::Kind::Ghost, ds::Size::M, glyph, move |a| {
                if let Some((chapter, series_)) = &target { a.pages.read_chapter(series_, chapter.clone()) }
            }).opacity(if can { 1. } else { 0.35 })
        };
        let mut controls = row().items_center().gap(t::SPACE_8);
        if let Some(counter) = &v.counter {
            controls = controls.kid(row().pad_xy(t::SPACE_10, t::SPACE_4).radius(t::R_PILL).items_center().bg(w(t::W12)).margin(0., 0., t::SPACE_8, 0.)
                .kid(line(counter, t::T_META, Font::Medium, w(t::W80))));
        }
        controls = controls.kid(self.zoom_controls(app, &v.zoom))
            .kid(step("reader-before", "chevron_left", v.before.clone()))
            .kid(step("reader-after", "chevron_right", v.after.clone()));
        // (the names beside Back, where a page's name sits in its head, up to the controls)
        let total = v.total;
        let controls_w = ds::Size::M.h() * 4. + t::SPACE_8 * 4. + t::SPACE_10 * 2. + ZOOM_WORDS_W + app.kit.g.measure(&format!("{total} / {total}"), &ts(t::T_META, Font::Medium, 0, 1), f32::INFINITY).0;
        let names_w = (view_w(app) - gutter * 2. - ds::BACK - t::SPACE_16 - controls_w - t::SPACE_16).max(0.);
        let names = col().at(gutter + ds::BACK + t::SPACE_16, t::SPACE_6, f32::NAN, f32::NAN).h(ds::Size::M.h()).w(names_w).center().decorative()
            .kid(text(&v.series, ts(t::T_PHONE_NAME, Font::Bold, t::C_WHITE, 1)).fill_w())
            .kid(text(&v.chapter, ts(t::T_NOTE, Font::Sans, t::C_MUTED, 1)).fill_w().margin(0., t::SPACE_2, 0., 0.));
        page_head(app, "reader-head", None, pos > 4., Some(controls)).kid(names)
    }

    /// The zoom in the bar, as a browser's: out, how much (a click: back to 100%), in; a step greyed where there's none.
    fn zoom_controls(&self, app: &App, zoom: &reader::Zoom) -> El {
        let middle = app.kit.height / 2.;
        let step = |key: &str, glyph: &'static str, inward: bool, can: bool| {
            ds::icon_button(app, key, ds::Kind::Ghost, ds::Size::M, glyph, move |a| Pages::zoom_reader(a, Some(inward), middle)).opacity(if can { 1. } else { 0.35 })
        };
        row().items_center()
            .kid(step("reader-zoom-out", "minus", false, zoom.can_out))
            .kid(tap(col().key("reader-zoom").w(ZOOM_WORDS_W).h(ds::Size::M.h()).center().kid(line(&zoom.shown, t::T_META, Font::Medium, w(t::W80))),
                t::PRESS_CHIP, move |a| Pages::zoom_reader(a, None, middle)))
            .kid(step("reader-zoom-in", "plus", true, zoom.can_in))
    }

    /// The chapter's foot: done, then back to the chapters and on to the next one.
    fn chapter_end(&self, app: &App, end: &reader::End, v: &reader::View) -> El {
        let mut buttons = row().gap(t::SPACE_10).items_center().margin(0., t::SPACE_20, 0., 0.)
            .kid(ds::button(app, "reader-chapters", ds::Kind::Secondary, ds::Size::L, None, &end.chapters, Pages::go_back));
        if let (Some(next), Some(series_)) = (v.after.clone(), self.manhwa.reading.clone()) {
            buttons = buttons.kid(ds::button_onward(app, "reader-next", ds::Kind::Primary, ds::Size::L, "chevron_right", 0., &v.next_label,
                move |a| a.pages.read_chapter(&series_, next.clone())));
        }
        col().key("reader-end").fill_w().items_center().pad4(t::SPACE_24, t::SPACE_32, t::SPACE_24, t::SPACE_28)
            .kid(line(&end.title, t::T_PHONE_SECTION, Font::Bold, t::C_WHITE))
            .kid(buttons)
    }
}

/// A grid of series' covers, as many a row as fit; only rows near the screen built. `view`: the window's height, and
/// how far down the grid the screen is.
fn comic_grid(scope: &str, cards: &[Card], content: f32, window: f32, view: (f32, f32)) -> El {
    let (columns, card) = grid_columns(content, window);
    let stride = card * t::S_POSTER_RATIO + t::S_CARD_GAP;
    let lines = cards.len().div_ceil(columns);
    let make = |index: usize| {
        let start = index * columns;
        row().key(&format!("{scope}-line-{index}")).fill_w().h(stride).no_shrink().items_start().gap(t::S_CARD_GAP).kids(
            cards[start..(start + columns).min(cards.len())].iter().map(|c| {
                let comic = c.comic.clone();
                comic_card(&format!("{scope}-{}", c.comic.path), c, card, move |a| a.pages.open_series(comic.clone()))
            }).collect(),
        )
    };
    col().key(&format!("{scope}-grid")).fill_w().kids(virtual_rows(lines, stride, view.0, view.1, make))
}

/// A series' cover, a poster's shape: its name on the art over the shaded foot, and the chapter you're up to (or its
/// rating).
fn comic_card(key: &str, c: &Card, width: f32, f: impl Fn(&mut App) + 'static) -> El {
    let h = width * t::S_POSTER_RATIO;
    let mut card = picture(col().key(key).size(width, h).no_shrink(), &c.picture, t::R_POSTER_CARD).bg(t::C_CARD);
    if let Some(art) = card.kids.last_mut().and_then(|art| art.image.as_mut()) {
        art.shade = 2;
    }
    let words_ = col().at(t::SPACE_14, f32::NAN, t::SPACE_14, t::SPACE_14).decorative()
        .kid(text(&c.title, ts(t::T_COMIC_TITLE, Font::Bold, t::C_INK, 2)).fill_w())
        .maybe(c.line.as_ref().map(|l| text(l, ts(t::T_META, Font::Medium, w(t::W75), 1)).fill_w().margin(0., t::SPACE_6, 0., 0.)));
    tap(card.kid(words_).grow_hover(1.03), t::PRESS_DEFAULT, f)
}

/// A series' Download all, by the core's rule for several at once (`zcore::downloads::batch`): every chapter not down
/// yet, with about what they'll take; then how far, Pause, Resume, Try again, and Downloaded (which opens Downloads).
fn download_all(app: &App, comic: &Comic, chapters: &[Chapter]) -> El {
    use zcore::downloads::Act;
    let items: Vec<(Option<State>, f32)> = chapters.iter().map(|c| downloads::progress_of(&c.path).map_or((None, 0.), |(s, f)| (Some(s), f))).collect();
    let (act, label) = zcore::downloads::batch(&items, false);
    let states: Vec<Option<State>> = items.iter().map(|i| i.0).collect();
    let targets: Vec<Chapter> = zcore::downloads::batch_targets(&states, act).into_iter().map(|i| chapters[i].clone()).collect();
    let chip = zcore::downloads::size_chip(act, downloads::chapters_size(&comic.path, chapters), targets.len(), chapters.len(), false);
    let comic = comic.clone();
    let button = ds::button(app, "series-download-all", ds::Kind::Secondary, ds::Size::L, Some(super::account::act_glyph(act)), label, move |a| match act {
        Act::Add => downloads::add_chapters(&comic, &targets),
        Act::Open => a.pages.push(Route::Downloads),
        _ => targets.iter().for_each(|c| downloads::act(&c.path, act)),
    });
    super::title::with_size_chip("series-download-all-wrap", button, chip)
}

/// A chapter as a list row: its still (a scene from it, made on the server, else made here from its pages), its name,
/// its line from the core (or how its download goes), and its download.
fn chapter_row(app: &App, comic: &Comic, r: &series::Row, width: f32, height: f32) -> El {
    let chapter = &r.chapter;
    let key = format!("chapter-{}", chapter.path);
    let hot = app.kit.hover_within(k(&key));
    let progress = downloads::progress_of(&chapter.path);
    let said = match progress {
        Some((state, _)) if state != State::Done => {
            let (done, total) = downloads::pages_of(&chapter.path);
            zcore::downloads::line(&zcore::downloads::Line { kind: downloads::Kind::Chapter, state: Some(state), size: total, done, online: !app.pages.offline, ..Default::default() })
        }
        _ => r.line.clone(),
    };
    let (dl_comic, dl_chapter) = (comic.clone(), chapter.clone());
    let (open_series, open_chapter) = (comic.clone(), chapter.clone());
    let control = super::account::download_control(app, &format!("chapter-dl-{}", chapter.path), &chapter.path, progress,
        Rc::new(move |_: &mut App| downloads::add_chapters(&dl_comic, std::slice::from_ref(&dl_chapter))),
        Rc::new(move |a: &mut App| a.pages.read_chapter(&open_series, open_chapter.clone())));
    let (series_, target) = (comic.clone(), chapter.clone());
    let still = if r.own_still { crate::images::chapter_still(&chapter.path).unwrap_or_else(|| r.still.clone()) } else { r.still.clone() };
    tap(
        row().key(&key).w(width).h(height).no_shrink().pad_xy(t::SPACE_12, 0.).items_center().radius(t::K_TITLE_ROW_RADIUS)
            .bg(if hot { w(t::W04) } else { 0 })
            .kid(picture(el().size(t::S_STILL_W, t::S_STILL_H).no_shrink(), &still, t::K_TITLE_STILL_RADIUS))
            .kid(col().grow().margin(t::SPACE_16, 0., t::SPACE_12, 0.)
                .kid(text(&chapter.label, ts(t::T_EPISODE_TITLE, Font::Bold, t::C_WHITE, 1)).fill_w())
                .kid(text(said, ts(t::T_TITLE_FACTS, Font::Sans, if r.reading { w(t::W85) } else { w(t::W50) }, 1)).fill_w().margin(0., t::SPACE_4, 0., 0.)))
            .kid(control),
        t::PRESS_ROW,
        move |a| a.pages.read_chapter(&series_, target.clone()),
    )
}

/// How wide the zoom's "125%" is: the widest it says, so stepping never moves the buttons.
const ZOOM_WORDS_W: f32 = t::K_MANHWA_ZOOM_WORDS;

/// The strip: each page as the core lays it out, cut into pieces on whole pixels (`Strip::pieces`), and only those near
/// the screen built (`pos`: how far down the strip the screen's top is). Each piece takes its place in the column and
/// draws its picture a pixel further, under the next.
fn strip_pieces(key: &str, chapter: &str, v: &reader::View, scale: f32, pos: f32, screen: f32) -> El {
    let (from, to) = (pos - screen, pos + screen * 2.);
    let sizes: Vec<(u32, u32)> = v.pages.iter().map(|p| (p.width, p.height)).collect();
    let strip = Strip::new(&sizes, v.column, scale);
    let width = v.width;
    // (the skeleton's grey under the whole strip, where a piece is still on its way; clipped, so the last piece's reach
    // stays inside it)
    let mut column = col().key(&format!("{key}-strip")).w(width).no_shrink().bg(t::C_SKELETON).clip();
    let mut built_to = 0.;
    for (i, page) in v.pages.iter().enumerate() {
        let (top, tall) = (page.top, page.tall);
        if top + tall < from || top > to {
            continue;
        }
        if top > built_to {
            column = column.kid(el().h(top - built_to).no_shrink());
        }
        let address = match &page.url {
            Some(url) => url.clone(),
            None => format!("file:{}", downloads::chapter_dir(chapter).join(&page.file).display()),
        };
        for (j, p) in strip.pieces(i, SLICE).into_iter().enumerate() {
            // (the picture alone, nothing behind it of its own: a piece's backing would cover the one above's reach)
            let mut art = el().key(&format!("{key}-{i}-{j}-art")).at(0., 0., f32::NAN, f32::NAN).size(width, p.drawn).decorative().image(address.clone());
            if let Some(image) = art.image.as_mut() {
                image.crop = Some((0, p.rows.0, page.width, p.rows.1));
                image.fade_ms = 0;
            }
            column = column.kid(el().key(&format!("{key}-{i}-{j}")).w(width).h(p.height).no_shrink().kid(art));
        }
        built_to = top + tall;
    }
    if v.height > built_to {
        column = column.kid(el().h(v.height - built_to).no_shrink());
    }
    column
}
