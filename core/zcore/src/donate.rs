//! Donations: voluntary, toward a goal the owner sets and keeps up by hand (nothing here takes payments). Its button
//! shows only while the owner has donations on; its panel shows the goal and the bank details to send to, each
//! copied with a tap (not on a TV, which has no clipboard worth the name).
//!
//! It lives as long as the app's signed-in pages: asked once the app is in, and again each time the panel opens.
//!
//! ```
//! use zcore::donate::{Donate, Event};
//! use zcore::root::Root;
//! use zcore::screen::{Clock, Device, Effect, Platform};
//!
//! let platform = Platform { device: Device::Phone, model: "Pixel".into(), version_name: "1.0".into(),
//!                           version_code: 1, os: "t".into(), stream: "s".into(), codecs: vec![], hdr: false };
//! let clock = Clock { now: 1_794_657_600., utc_offset: 0 };
//! let mut root = Root::new(platform, "https://host.invalid".into(), clock);
//! let page = root.open(Donate::default());
//! let turn = root.update::<Donate>(page, Event::Opened, clock);
//! assert!(matches!(&turn.orders[0].effect,
//!                  Effect::Request { request, .. } if request.url == "https://host.invalid/api/donate"));
//! ```

use crate::api::{self, Outcome};
use crate::screen::{AppEvent, Asks, Context, Device, Effect, Reply, Screen};
use serde::Deserialize;

/// The server's answer: the owner's donation settings.
pub const PATH: &str = "/api/donate";
/// How long a copied detail's button says so before it's a Copy button again.
pub const COPIED_MS: u32 = 1_500;

/// What happens to donations.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The app is in: asked for the first time.
    Opened,
    /// The panel opened: asked again, for the latest amount raised.
    Refresh,
    /// The detail at `index` was copied to the clipboard.
    Copied { index: u32 },
    /// Show (or hide) bank details pressed.
    Bank,
}

/// Exactly what the Donate button and its panel draw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// Donations are on: the button shows.
    pub on: bool,
    /// The button's label, beside its heart.
    pub button: String,
    /// The panel's line, under the wordmark.
    pub line: String,
    /// How far toward the goal; None when there's no goal.
    pub goal: Option<Goal>,
    /// The bank details, folded away until asked for: the button's words ("Show bank details", "Hide bank details").
    pub bank: String,
    pub bank_open: bool,
    /// The details: the names (account name, bank) side by side in one row, each number a row of its own.
    pub details: Vec<Detail>,
    /// A TV: no copying, and the values large.
    pub tv: bool,
}

/// The goal as a meter: what's in ("PKR 12,000") and of how much ("of PKR 30,000") over the bar.
#[derive(Clone, Debug, PartialEq)]
pub struct Goal {
    /// Raised over the goal, 0 to 1.
    pub share: f32,
    pub raised: String,
    pub of: String,
}

/// A bank detail: what it is, the value to send to, and its Copy button.
#[derive(Clone, Debug, PartialEq)]
pub struct Detail {
    pub label: String,
    pub value: String,
    /// The button's words: "Copy", or "Copied" for a moment after it's pressed.
    pub copy: String,
    pub copied: bool,
    /// It has a Copy button: a number to type (an account number, an IBAN), not a name; never on a TV.
    pub copyable: bool,
    /// A name (no digits: the account's name, the bank's): drawn beside the others in one row.
    pub name: bool,
}

/// The server's settings, as `/api/donate` sends them.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
struct Settings {
    #[serde(default)]
    on: bool,
    #[serde(default)]
    goal: u64,
    #[serde(default)]
    raised: u64,
    #[serde(default)]
    details: Vec<Line>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
struct Line {
    #[serde(default)]
    label: String,
    #[serde(default)]
    value: String,
}

#[derive(Debug)]
enum Ask {
    Settings,
    /// The copied detail's moment is up.
    Copied,
}

/// Donations' model.
#[derive(Debug, Default)]
pub struct Donate {
    asks: Asks<Ask>,
    settings: Settings,
    /// The detail just copied, and its timer.
    copied: Option<(u32, u32)>,
    /// The bank details are showing.
    bank_open: bool,
    tv: bool,
}

/// `n` with thousands separators: 30000 as "30,000".
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Donate {
    fn ask(&mut self, cx: &Context) -> Vec<Effect> {
        let id = self.asks.ask(Ask::Settings);
        vec![Effect::Request { id, request: api::get(&cx.base, &cx.who(), PATH, None) }]
    }

    /// Always in rupees: the owner's account is in Pakistan.
    fn money(n: u64) -> String {
        format!("PKR {}", thousands(n))
    }
}

impl Screen for Donate {
    type Event = Event;
    type View = View;

    fn update(&mut self, event: Event, cx: &Context) -> Vec<Effect> {
        self.tv = cx.platform.device == Device::Tv;
        match event {
            Event::Opened => self.ask(cx),
            // (the panel opened: the bank details folded again)
            Event::Refresh => {
                self.bank_open = false;
                self.ask(cx)
            }
            Event::Bank => {
                self.bank_open = !self.bank_open;
                vec![]
            }
            Event::Copied { .. } if self.tv => vec![],
            Event::Copied { index } => {
                let mut effects: Vec<Effect> = self.copied.take().map(|(_, timer)| Effect::Cancel { id: timer }).into_iter().collect();
                let timer = self.asks.ask(Ask::Copied);
                self.copied = Some((index, timer));
                effects.push(Effect::Timer { id: timer, ms: COPIED_MS });
                effects
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        match self.asks.take(id) {
            Some(Ask::Copied) => {
                if self.copied.is_some_and(|(_, timer)| timer == id) {
                    self.copied = None;
                }
                vec![]
            }
            Some(Ask::Settings) => match api::outcome(reply, cx.online) {
                Outcome::Body { body, .. } => match serde_json::from_str::<Settings>(&body) {
                    Ok(settings) => {
                        self.settings = settings;
                        vec![]
                    }
                    Err(e) => vec![Effect::Report { place: "donate".into(), message: e.to_string() }],
                },
                // (offline or refused: what was shown stays; it's asked again next time the panel opens)
                Outcome::Unchanged | Outcome::Failed { .. } => vec![],
            },
            None => vec![],
        }
    }

    fn app(&mut self, event: &AppEvent, _cx: &Context) -> Vec<Effect> {
        if let AppEvent::Account { account: None } = event {
            // (signed out: nothing of the last account's shown; ids keep counting, so a late answer is ignored)
            self.asks.forget();
            self.settings = Settings::default();
            self.copied = None;
        }
        vec![]
    }

    fn view(&self) -> View {
        let s = &self.settings;
        let copied = self.copied.map(|(index, _)| index as usize);
        View {
            on: s.on,
            button: "Donate".into(),
            line: "Help keep zunflix running.".into(),
            goal: (s.goal > 0).then(|| Goal {
                share: (s.raised as f64 / s.goal as f64).min(1.) as f32,
                raised: Self::money(s.raised),
                of: format!("of {}", Self::money(s.goal)),
            }),
            bank: if self.bank_open { "Hide bank details" } else { "Show bank details" }.into(),
            bank_open: self.bank_open,
            details: s
                .details
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let done = copied == Some(i);
                    let name = !d.value.chars().any(|c| c.is_ascii_digit());
                    Detail {
                        label: d.label.clone(),
                        value: d.value.clone(),
                        copy: if done { "Copied" } else { "Copy" }.into(),
                        copied: done,
                        copyable: !self.tv && !name,
                        name,
                    }
                })
                .collect(),
            tv: self.tv,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::{Root, ScreenId};
    use crate::screen::{response, test_context, Clock, Platform};

    const NOW: f64 = 1_794_657_600.;
    const CLOCK: Clock = Clock { now: NOW, utc_offset: 0 };
    const ON: &str = r#"{"on": true, "goal": 30000, "raised": 12000,
        "details": [{"label": "Bank", "value": "Meezan"}, {"label": "IBAN", "value": "PK36MEZN0001"}]}"#;

    fn open(device: Device, body: &str) -> (Root, ScreenId) {
        let platform = Platform { device, ..test_context(NOW).platform };
        let mut root = Root::new(platform, "https://host.invalid".into(), CLOCK);
        let id = root.open(Donate::default());
        let turn = root.update::<Donate>(id, Event::Opened, CLOCK);
        let [order] = turn.orders.as_slice() else { panic!("expected one order") };
        let Effect::Request { id: ask, request } = &order.effect else { panic!("expected a Request") };
        assert_eq!(request.url, "https://host.invalid/api/donate");
        root.answer(*ask, response(200, body, None), CLOCK);
        (root, id)
    }

    fn view(root: &Root, id: ScreenId) -> View {
        root.view::<Donate>(id).unwrap_or_default()
    }

    #[test]
    fn thousands_are_separated() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(30000), "30,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn the_settings_are_read_with_their_words() {
        let (root, id) = open(Device::Phone, ON);
        let view = view(&root, id);
        assert!(view.on && !view.tv);
        assert_eq!(view.goal, Some(Goal { share: 0.4, raised: "PKR 12,000".into(), of: "of PKR 30,000".into() }));
        assert_eq!(
            view.details[1],
            Detail { label: "IBAN".into(), value: "PK36MEZN0001".into(), copy: "Copy".into(), copied: false, copyable: true, name: false }
        );
        assert_eq!((view.line.as_str(), view.button.as_str(), view.bank.as_str()), ("Help keep zunflix running.", "Donate", "Show bank details"));
    }

    #[test]
    fn bank_details_fold_and_fold_again_as_the_panel_opens() {
        let (mut root, id) = open(Device::Phone, ON);
        root.update::<Donate>(id, Event::Bank, CLOCK);
        assert_eq!((view(&root, id).bank_open, view(&root, id).bank.as_str()), (true, "Hide bank details"));
        root.update::<Donate>(id, Event::Refresh, CLOCK);
        assert!(!view(&root, id).bank_open);
    }

    #[test]
    fn off_no_goal_and_past_the_goal() {
        let (root, id) = open(Device::Phone, r#"{"on": false}"#);
        assert!(!view(&root, id).on);
        let (root, id) = open(Device::Phone, r#"{"on": true, "goal": 0, "raised": 500, "details": []}"#);
        assert_eq!(view(&root, id).goal, None);
        let (root, id) = open(Device::Phone, r#"{"on": true, "goal": 1000, "raised": 2500, "details": []}"#);
        assert_eq!(view(&root, id).goal, Some(Goal { share: 1., raised: "PKR 2,500".into(), of: "of PKR 1,000".into() }));
    }

    #[test]
    fn an_unreadable_answer_is_reported_and_a_failure_keeps_what_was_shown() {
        let (mut root, id) = open(Device::Phone, ON);
        let turn = root.update::<Donate>(id, Event::Refresh, CLOCK);
        let Effect::Request { id: ask, .. } = turn.orders[0].effect.clone() else { panic!("expected a Request") };
        root.answer(ask, Reply::Unreachable { offline: true, message: "x".into() }, CLOCK);
        assert!(view(&root, id).on);
        let turn = root.update::<Donate>(id, Event::Refresh, CLOCK);
        let Effect::Request { id: ask, .. } = turn.orders[0].effect.clone() else { panic!("expected a Request") };
        let turn = root.answer(ask, response(200, "<html>", None), CLOCK);
        assert!(turn.orders.iter().any(|o| matches!(&o.effect, Effect::Report { place, .. } if place == "donate")));
    }

    #[test]
    fn copied_says_so_for_a_moment() {
        let (mut root, id) = open(Device::Phone, ON);
        let turn = root.update::<Donate>(id, Event::Copied { index: 0 }, CLOCK);
        let [order] = turn.orders.as_slice() else { panic!("expected one order") };
        let Effect::Timer { id: first, ms } = order.effect else { panic!("expected a Timer") };
        assert_eq!(ms, COPIED_MS);
        assert_eq!(view(&root, id).details[0].copy, "Copied");
        // (another copied: the first is a Copy button again, and its timer goes)
        let turn = root.update::<Donate>(id, Event::Copied { index: 1 }, CLOCK);
        assert!(matches!(turn.orders[0].effect, Effect::Cancel { id } if id == first));
        let Effect::Timer { id: second, .. } = turn.orders[1].effect else { panic!("expected a Timer") };
        let view_now = view(&root, id);
        assert!(!view_now.details[0].copied && view_now.details[1].copied);
        root.answer(second, Reply::Fired, CLOCK);
        assert!(view(&root, id).details.iter().all(|d| !d.copied && d.copy == "Copy"));
    }

    #[test]
    fn a_tv_has_nothing_to_copy() {
        let (mut root, id) = open(Device::Tv, ON);
        let v = view(&root, id);
        assert!(v.tv && v.details.iter().all(|d| !d.copyable));
        assert!(root.update::<Donate>(id, Event::Copied { index: 0 }, CLOCK).orders.is_empty());
        assert!(!view(&root, id).details[0].copied);
    }
}
