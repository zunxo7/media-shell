//! Watch history: everything watched, newest first, under its day, narrowed by what's typed; a tap plays it from
//! where it was left.
//!
//! ```
//! use zcore::history::{Event, History};
//! use zcore::root::Root;
//! use zcore::screen::{Clock, Device, Effect, Platform, Reply};
//!
//! let platform = Platform { device: Device::Tv, model: "TV".into(), version_name: "1.0".into(), version_code: 1,
//!                           os: "t".into(), stream: "s".into(), codecs: vec![], hdr: false };
//! let clock = Clock { now: 1_794_657_600., utc_offset: 0 };
//! let mut root = Root::new(platform, "https://host.invalid".into(), clock);
//! let page = root.open(History::default());
//! let turn = root.update::<History>(page, Event::Opened { profile: "p1".into() }, clock);
//! let Some(Effect::Load { id, .. }) = turn.orders.first().map(|o| o.effect.clone()) else { panic!() };
//! let turn = root.answer(id, Reply::Missing, clock);
//! let Some(Effect::Request { id, request }) = turn.orders.first().map(|o| o.effect.clone()) else { panic!() };
//! assert_eq!(request.url, "https://host.invalid/api/progress");
//! let body = r#"{"m": {"show": "m", "title": "Dune", "t": 60, "d": 9000, "updated": 1794657000}}"#;
//! root.answer(id, Reply::Response { status: 200, headers: vec![], body: body.into() }, clock);
//! let view = root.view::<History>(page).unwrap_or_default();
//! assert_eq!(view.days[0].heading, "Today");
//! assert_eq!(view.days[0].rows[0].line, "11:50 am · 2h 29m left");
//! ```

use crate::dates::{day, day_heading, time_of_day};
use crate::progress::{self, Saved};
use crate::screen::{AppEvent, Context, Effect, Empty, Glyph, Reply, Screen, Source, Trouble};
use crate::search::fold;
use crate::title::left_words;

/// What happens on the history page.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The page opened for `profile`.
    Opened { profile: String },
    /// What's watched may have changed (the player saved a spot): asked for again.
    Changed,
    /// What's typed in its search; "" when search is closed.
    Query { text: String },
    /// A row pressed (its key).
    Tapped { key: String },
    /// Try again, after it couldn't load.
    Retry,
}

/// Exactly what the history page draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub title: String,
    /// Its search field's hint.
    pub hint: String,
    /// How many things were watched ("12 watched"), once there are any.
    pub count: Option<String>,
    /// Nothing to show yet: the page's skeleton.
    pub loading: bool,
    pub days: Vec<Day>,
    pub empty: Option<Empty>,
    pub trouble: Option<Trouble>,
}

/// A day's heading and what was watched on it, newest first.
#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    pub heading: String,
    pub rows: Vec<Row>,
}

/// One thing watched.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// What pressing it sends back ([`Event::Tapped`]).
    pub key: String,
    pub title: String,
    /// "E3 · 9:41 pm · 12m left".
    pub line: String,
    pub picture: Option<String>,
    /// How far along, 0 to 1; None when its length isn't known.
    pub progress: Option<f32>,
}

/// The history page's model.
#[derive(Debug)]
pub struct History {
    source: Source,
    /// The profile it's open for; None until it opens.
    profile: Option<String>,
    now: f64,
    utc_offset: i32,
    query: String,
    /// What was watched, newest first.
    spots: Vec<Saved>,
    /// The body `spots` were read from.
    read: Option<String>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            source: Source::new(progress::PATH, ""),
            profile: None,
            now: 0.,
            utc_offset: 0,
            query: String::new(),
            spots: Vec::new(),
            read: None,
        }
    }
}

impl History {
    /// The spots read again when the page's body has changed; an unreadable one reported.
    fn read(&mut self) -> Vec<Effect> {
        if self.source.shown == self.read {
            return vec![];
        }
        self.read = self.source.shown.clone();
        let Some(body) = &self.read else { return vec![] };
        match progress::saved(body) {
            Ok(mut spots) => {
                // (never started, or with no name to show: not something watched)
                spots.retain(|s| s.at > 0. && !s.show.is_empty() && !s.title.is_empty());
                spots.sort_by(|a, b| b.updated.total_cmp(&a.updated));
                self.spots = spots;
                vec![]
            }
            Err(e) => {
                self.spots.clear();
                vec![Effect::Report { place: "history".into(), message: e.to_string() }]
            }
        }
    }

    fn row(&self, s: &Saved) -> Row {
        let line = [s.ep.map(|n| format!("E{n}")), Some(time_of_day(s.updated, self.utc_offset)), Some(left_words(s.at, s.length))];
        Row {
            key: s.key.clone(),
            title: s.title.clone(),
            line: line.into_iter().flatten().collect::<Vec<_>>().join(" · "),
            picture: s.cover.clone(),
            progress: (s.length > 0.).then(|| (s.at / s.length).clamp(0., 1.) as f32),
        }
    }
}

impl Screen for History {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        match event {
            Event::Opened { profile } => {
                if self.profile.as_ref() != Some(&profile) {
                    let source = Source::new(progress::PATH, &progress::key(&profile));
                    *self = Self { source, profile: Some(profile), ..Self::default() };
                }
                (self.now, self.utc_offset) = (cx.clock.now, cx.clock.utc_offset);
                self.source.open(cx)
            }
            // (before the page has opened there's nothing to ask again)
            Event::Changed | Event::Retry if self.profile.is_none() => vec![],
            Event::Changed | Event::Retry => self.source.refresh(cx),
            Event::Query { text } => {
                self.query = text;
                vec![]
            }
            Event::Tapped { key } => self
                .spots
                .iter()
                .find(|s| s.key == key)
                .map(|s| Effect::Play { show: s.show.clone(), episode: (s.key != s.show).then(|| s.key.clone()) })
                .into_iter()
                .collect(),
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, _cx: &Context) -> Vec<Effect> {
        let Some(mut effects) = self.source.answer(id, reply) else { return vec![] };
        effects.extend(self.read());
        effects
    }

    fn app(&mut self, event: &AppEvent, cx: &Context) -> Vec<Effect> {
        match event {
            AppEvent::PageChanged { key } => self.source.changed(key),
            // (the player saved its last spot: the list is asked for again)
            AppEvent::Watched if self.profile.is_some() => self.source.refresh(cx),
            _ => vec![],
        }
    }

    fn view(&self) -> View {
        let words = fold(self.query.trim());
        let today = day(self.now, self.utc_offset);
        let mut days: Vec<Day> = Vec::new();
        for s in self.spots.iter().filter(|s| words.is_empty() || fold(&s.title).contains(&words)) {
            let heading = day_heading(day(s.updated, self.utc_offset), today);
            match days.last_mut() {
                Some(d) if d.heading == heading => d.rows.push(self.row(s)),
                _ => days.push(Day { heading, rows: vec![self.row(s)] }),
            }
        }
        let trouble = self.source.trouble();
        let loading = self.source.shown.is_none() && trouble.is_none();
        let empty = (days.is_empty() && !loading && trouble.is_none()).then(|| {
            if words.is_empty() {
                Empty {
                    glyph: Glyph::History,
                    title: "Nothing watched yet".into(),
                    line: "What you watch shows here, day by day.".into(),
                }
            } else {
                Empty {
                    glyph: Glyph::Search,
                    title: "Nothing found".into(),
                    line: "Nothing you've watched goes by that name.".into(),
                }
            }
        });
        View {
            title: "Watch history".into(),
            hint: "Search watch history".into(),
            count: (!self.spots.is_empty()).then(|| format!("{} watched", self.spots.len())),
            loading,
            days,
            empty,
            trouble,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::{Root, ScreenId, Turn};
    use crate::screen::{response, test_context, Clock};

    const NOW: f64 = 1_794_657_600.; // 14 Nov 2026 12:00 UTC
    const HOUR: f64 = 3600.;
    const CLOCK: Clock = Clock { now: NOW, utc_offset: 0 };

    /// Watch history on the app's root, its reads and requests answered from a list.
    struct Page {
        root: Root,
        id: ScreenId,
    }

    impl Page {
        fn new() -> Self {
            let mut root = Root::new(test_context(NOW).platform, "https://host.invalid".into(), CLOCK);
            let id = root.open(History::default());
            Self { root, id }
        }

        /// An event, its reads answered with `kept` and its requests with `answers` in order; what else it asked for.
        fn send(&mut self, event: Event, kept: Reply, answers: Vec<Reply>) -> Vec<Effect> {
            let turn = self.root.update::<History>(self.id, event, CLOCK);
            self.run(turn, kept, answers)
        }

        fn run(&mut self, turn: Turn, kept: Reply, answers: Vec<Reply>) -> Vec<Effect> {
            let mut answers = answers.into_iter();
            let mut queue: Vec<Effect> = turn.orders.into_iter().map(|o| o.effect).collect();
            let mut done = Vec::new();
            while !queue.is_empty() {
                let effect = queue.remove(0);
                let turn = match &effect {
                    Effect::Load { id, key } => {
                        assert!(key.starts_with("me-progress-"), "{key}");
                        self.root.answer(*id, kept.clone(), CLOCK)
                    }
                    Effect::Request { id, request } => {
                        assert_eq!(request.url, "https://host.invalid/api/progress");
                        let reply = answers.next().unwrap_or(Reply::Unreachable { offline: true, message: "x".into() });
                        self.root.answer(*id, reply, CLOCK)
                    }
                    _ => {
                        done.push(effect);
                        continue;
                    }
                };
                queue.extend(turn.orders.into_iter().map(|o| o.effect));
            }
            done
        }

        fn view(&self) -> View {
            self.root.view::<History>(self.id).unwrap_or_default()
        }
    }

    /// The page opened, the kept copy `kept`, the server answering `answers`.
    fn open(kept: Reply, answers: Vec<Reply>) -> (Page, Vec<Effect>) {
        let mut page = Page::new();
        let done = page.send(Event::Opened { profile: "p1".into() }, kept, answers);
        (page, done)
    }

    fn body() -> String {
        serde_json::json!({
            "ep1": {"show": "show", "title": "Severance", "ep": 3, "t": 1500, "d": 3000, "updated": NOW - HOUR},
            "film": {"show": "film", "title": "Dune", "t": 8990, "d": 9000, "updated": NOW - 25. * HOUR},
            "old": {"show": "old", "title": "Amélie", "t": 60, "d": 0, "updated": NOW - 400. * 24. * HOUR},
            "unstarted": {"show": "u", "title": "Never", "t": 0, "d": 100, "updated": NOW},
            "nameless": {"show": "", "title": "", "t": 5, "d": 100, "updated": NOW},
        })
        .to_string()
    }

    #[test]
    fn grouped_by_day_newest_first_with_their_words() {
        let (page, done) = open(Reply::Missing, vec![response(200, &body(), Some("e"))]);
        assert_eq!(done, vec![Effect::Store { key: "me-progress-p1".into(), body: body(), etag: Some("e".into()) }]);
        let view = page.view();
        assert_eq!(view.count.as_deref(), Some("3 watched"));
        let headings: Vec<_> = view.days.iter().map(|d| d.heading.as_str()).collect();
        assert_eq!(headings, ["Today", "Yesterday", "10th October 2025"]);
        assert_eq!(
            view.days[0].rows[0],
            Row {
                key: "ep1".into(),
                title: "Severance".into(),
                line: "E3 · 11:00 am · 25m left".into(),
                picture: None,
                progress: Some(0.5),
            }
        );
        assert_eq!(view.days[1].rows[0].line, "11:00 am · Watched");
        assert_eq!(view.days[2].rows[0].progress, None);
        assert!(!view.loading && view.empty.is_none() && view.trouble.is_none());
    }

    #[test]
    fn search_folds_accents_and_says_when_nothing_matches() {
        let (mut page, _) = open(Reply::Missing, vec![response(200, &body(), None)]);
        page.send(Event::Query { text: " amelie ".into() }, Reply::Missing, vec![]);
        assert_eq!(page.view().days[0].rows[0].title, "Amélie");
        page.send(Event::Query { text: "zzz".into() }, Reply::Missing, vec![]);
        let view = page.view();
        assert!(view.days.is_empty());
        assert_eq!(view.empty.map(|e| e.glyph), Some(Glyph::Search));
        assert_eq!(view.count.as_deref(), Some("3 watched"));
    }

    #[test]
    fn a_tap_plays_the_episode_or_the_film() {
        let (mut page, _) = open(Reply::Missing, vec![response(200, &body(), None)]);
        assert_eq!(
            page.send(Event::Tapped { key: "ep1".into() }, Reply::Missing, vec![]),
            vec![Effect::Play { show: "show".into(), episode: Some("ep1".into()) }]
        );
        assert_eq!(
            page.send(Event::Tapped { key: "film".into() }, Reply::Missing, vec![]),
            vec![Effect::Play { show: "film".into(), episode: None }]
        );
        assert!(page.send(Event::Tapped { key: "gone".into() }, Reply::Missing, vec![]).is_empty());
    }

    #[test]
    fn loading_then_empty() {
        let mut page = Page::new();
        page.root.update::<History>(page.id, Event::Opened { profile: "p".into() }, CLOCK);
        assert!(page.view().loading);
        let (page, _) = open(Reply::Missing, vec![response(200, "{}", None)]);
        let view = page.view();
        assert!(!view.loading && view.count.is_none());
        assert_eq!(view.empty.map(|e| e.title), Some("Nothing watched yet".into()));
    }

    #[test]
    fn a_kept_copy_shows_at_once_and_offline_with_nothing_kept_is_trouble() {
        // (kept 10 minutes ago: past its minute fresh, so shown while asked again; the ask fails, the copy stays)
        let kept = Reply::Kept { body: body(), etag: None, at: NOW - 600. };
        let (page, _) = open(kept, vec![Reply::Unreachable { offline: true, message: "offline".into() }]);
        assert_eq!(page.view().days.len(), 3);
        assert_eq!(page.view().trouble, None);

        let (mut page, _) =
            open(Reply::Missing, vec![Reply::Unreachable { offline: true, message: "Unable to resolve host".into() }]);
        let view = page.view();
        assert!(view.days.is_empty() && view.empty.is_none() && !view.loading);
        assert_eq!(view.trouble, Some(Trouble { message: crate::api::OFFLINE.into(), offline: true, busy: false }));
        let retry = page.root.update::<History>(page.id, Event::Retry, CLOCK);
        assert!(matches!(retry.orders.as_slice(), [o] if matches!(o.effect, Effect::Request { .. })));
        assert_eq!(page.view().trouble.map(|t| t.busy), Some(true));
    }

    #[test]
    fn a_fresh_copy_is_used_and_changed_asks_again() {
        let kept = Reply::Kept { body: "{}".into(), etag: Some("e".into()), at: NOW - 5. };
        let (mut page, done) = open(kept, vec![]);
        assert!(done.is_empty());
        assert!(page.view().empty.is_some());
        let changed = page.root.update::<History>(page.id, Event::Changed, CLOCK);
        let [order] = changed.orders.as_slice() else { panic!("expected one order") };
        let Effect::Request { request, .. } = &order.effect else { panic!("expected a Request") };
        assert_eq!(crate::screen::header(&request.headers, "If-None-Match"), Some("e"));
        page.run(changed, Reply::Missing, vec![response(200, &body(), None)]);
        assert_eq!(page.view().count.as_deref(), Some("3 watched"));
    }

    #[test]
    fn a_watched_title_asks_again() {
        let kept = Reply::Kept { body: "{}".into(), etag: Some("e".into()), at: NOW - 5. };
        let (mut page, _) = open(kept, vec![]);
        let turn = page.root.app(AppEvent::Watched, CLOCK);
        assert!(matches!(turn.orders.as_slice(), [o] if matches!(o.effect, Effect::Request { .. })));
    }

    #[test]
    fn an_unreadable_answer_is_reported() {
        let (page, done) = open(Reply::Missing, vec![response(200, "<html>", None)]);
        assert!(done.iter().any(|e| matches!(e, Effect::Report { place, .. } if place == "history")));
        assert!(page.view().empty.is_some());
    }

    #[test]
    fn another_profile_starts_afresh() {
        let (mut page, _) = open(Reply::Missing, vec![response(200, &body(), None)]);
        let turn = page.root.update::<History>(page.id, Event::Opened { profile: "p2".into() }, CLOCK);
        assert!(matches!(turn.orders.as_slice(), [o] if matches!(&o.effect, Effect::Load { key, .. } if key == "me-progress-p2")));
        assert!(page.view().loading);
    }

    #[test]
    fn closed_mid_request_the_page_still_comes_for_the_next_screen() {
        let mut page = Page::new();
        let opened = page.root.update::<History>(page.id, Event::Opened { profile: "p1".into() }, CLOCK);
        let [order] = opened.orders.as_slice() else { panic!("expected one order") };
        let Effect::Load { id, .. } = order.effect.clone() else { panic!("expected a Load") };
        let kept = Reply::Kept { body: body(), etag: Some("e".into()), at: NOW - 600. };
        let asked = page.root.answer(id, kept, CLOCK);
        assert_eq!(page.view().days.len(), 3);
        let [order] = asked.orders.as_slice() else { panic!("expected one order") };
        let Effect::Request { id, request } = order.effect.clone() else { panic!("expected a Request") };
        assert!(request.cookies);
        // (the page is the store's, not the screen's: closing the screen leaves its request to finish and be kept)
        assert_eq!(page.root.close(page.id, CLOCK), Turn::default());
        let late = page.root.answer(id, response(200, "{}", None), CLOCK);
        assert!(late.changed.is_empty());
        assert!(matches!(late.orders.as_slice(), [o] if matches!(o.effect, Effect::Store { .. })));
    }
}
