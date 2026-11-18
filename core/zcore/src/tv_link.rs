//! Signing a TV in from a phone. The TV asks the server for a code ([`TvCode`]: the code, and a QR code of
//! host.invalid/link?code=…), and asks every 2 s whether a phone has approved it; a phone or a PC types that code in
//! ([`SignInTv`]) and approves it for its account; the TV is then signed in.
//!
//! ```
//! use zcore::tv_link::clean_code;
//! assert_eq!(clean_code("ab-c 12d9x"), "ABC12D");
//! ```

use crate::api::{self, Outcome};
use crate::screen::{Asks, Context, Effect, Reply, Screen};
use crate::sections::encode;
use serde_json::{json, Value};

/// How many letters and numbers a code has (`server.py`'s `tvlink_start`).
pub const CODE_LENGTH: usize = 6;
/// The longest name a TV is given.
pub const NAME_MOST: usize = 40;
/// How often the TV asks whether its code was approved.
pub const POLL_MS: u32 = 2_000;
/// How long the TV waits before asking for a code again after it couldn't get one.
pub const RETRY_MS: u32 = 5_000;
/// How long a code lasts when the server doesn't say.
const CODE_LIFE: f64 = 600.;

/// A code as it's typed: letters upper case, anything but letters and numbers dropped, no longer than a code.
pub fn clean_code(text: &str) -> String {
    text.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).take(CODE_LENGTH).collect()
}

// ---------- the phone: Sign in a TV ----------

/// Which part of Sign in a TV shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    /// The name and the code.
    #[default]
    Code,
    /// How to get the app onto a TV.
    Steps,
    /// Approved: the TV signs itself in.
    Done,
}

/// What happens on Sign in a TV.
#[derive(Clone, Debug, PartialEq)]
pub enum SignInEvent {
    /// The sheet opened (again): from the start.
    Opened,
    /// "See how to install it".
    Steps,
    /// "Back to the code".
    Back,
    /// The code changed as it's typed: what went wrong is put away.
    Typed,
    /// Sign in TV, with what's typed.
    Submit { name: String, code: String },
}

/// One of the steps to get the app on a TV: its number, and its words with the part to type in bold.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub number: String,
    pub before: String,
    pub bold: String,
    pub after: String,
}

/// Exactly what Sign in a TV draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignInView {
    pub stage: Stage,
    pub title: String,
    pub line: String,
    pub name_hint: String,
    pub code_hint: String,
    pub button: String,
    pub busy: bool,
    pub error: Option<String>,
    /// "No app on the TV yet? " and the link after it.
    pub no_app: String,
    pub how: String,
    pub steps: Vec<Step>,
    pub back: String,
    pub done: String,
}

/// Sign in a TV's model.
#[derive(Debug, Default)]
pub struct SignInTv {
    asks: Asks<()>,
    stage: Stage,
    busy: bool,
    error: Option<String>,
}

impl Screen for SignInTv {
    type Event = SignInEvent;
    type View = SignInView;

    fn update(&mut self, event: SignInEvent, cx: &Context) -> Vec<Effect> {
        match event {
            SignInEvent::Opened => {
                self.asks.forget();
                *self = Self { asks: std::mem::take(&mut self.asks), ..Self::default() };
                vec![]
            }
            SignInEvent::Steps => {
                self.stage = Stage::Steps;
                vec![]
            }
            SignInEvent::Back => {
                self.stage = Stage::Code;
                vec![]
            }
            SignInEvent::Typed => {
                self.error = None;
                vec![]
            }
            SignInEvent::Submit { name, code } => {
                if self.busy || self.stage != Stage::Code {
                    return vec![];
                }
                let code = clean_code(&code);
                if code.len() < CODE_LENGTH {
                    self.error = Some(format!("The code is the {CODE_LENGTH} letters and numbers on the TV."));
                    return vec![];
                }
                let name: String = name.trim().chars().take(NAME_MOST).collect();
                // (no name: the server calls it "Android TV")
                let body = if name.is_empty() { json!({"code": code}) } else { json!({"code": code, "name": name}) };
                self.busy = true;
                self.error = None;
                let request = api::post(&cx.base, &cx.who(), "/api/tvlink/approve", body.to_string());
                vec![Effect::Request { id: self.asks.ask(()), request }]
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        if self.asks.take(id).is_none() {
            return vec![];
        }
        self.busy = false;
        match api::outcome(reply, cx.online) {
            Outcome::Failed { failure } => self.error = Some(failure.words),
            _ => self.stage = Stage::Done,
        }
        vec![]
    }

    fn view(&self) -> SignInView {
        let (title, line) = match self.stage {
            Stage::Code => ("Sign in a TV", "Type the code on the TV's sign-in screen."),
            Stage::Steps => ("On your TV", "For Android TV and Google TV."),
            Stage::Done => ("Your TV is signing in", "It opens in a moment."),
        };
        let step = |n: u32, before: &str, bold: &str, after: &str| Step {
            number: n.to_string(),
            before: before.into(),
            bold: bold.into(),
            after: after.into(),
        };
        SignInView {
            stage: self.stage,
            title: title.into(),
            line: line.into(),
            name_hint: "Name (e.g. Living room)".into(),
            code_hint: "Code".into(),
            button: if self.busy { "Signing in…" } else { "Sign in TV" }.into(),
            busy: self.busy,
            error: self.error.clone(),
            no_app: "No app on the TV yet? ".into(),
            how: "See how to install it".into(),
            steps: vec![
                step(1, "On the TV, open a web browser, or Downloader by AFTVnews from the Play Store.", "", ""),
                step(2, "Go to ", "host.invalid/get", "."),
                step(3, "Allow it to install apps, then choose Install.", "", ""),
                step(4, "Open zunflix and type its code here.", "", ""),
            ],
            back: "Back to the code".into(),
            done: "Done".into(),
        }
    }
}

// ---------- the TV: its code ----------

/// What happens on the TV's code page.
#[derive(Clone, Debug, PartialEq)]
pub enum CodeEvent {
    /// The page opened: a code is asked for.
    Opened,
}

/// Exactly what the TV's code page draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CodeView {
    pub hint: String,
    /// What the QR code holds, once there's a code.
    pub qr: Option<String>,
    /// The code, or why there's none; empty while it comes.
    pub code: String,
    /// No code yet and nothing wrong: a spinner where the QR code goes.
    pub spinner: bool,
    /// A phone approved it: this TV is signed in now.
    pub signed_in: bool,
}

#[derive(Debug)]
enum Ask {
    Start,
    Poll,
    /// The wait before the next poll.
    Tick,
    /// The wait before asking for a code again.
    Retry,
}

/// A code the TV shows: `until`, when it runs out (seconds since 1970).
#[derive(Debug)]
struct Link {
    token: String,
    code: String,
    url: String,
    until: f64,
}

/// The TV's code page's model.
#[derive(Debug, Default)]
pub struct TvCode {
    asks: Asks<Ask>,
    link: Option<Link>,
    failed: bool,
    signed_in: bool,
}

impl TvCode {
    fn start(&mut self, cx: &Context) -> Vec<Effect> {
        let request = api::post(&cx.base, &cx.who(), "/api/tvlink/start", "{}".into());
        vec![Effect::Request { id: self.asks.ask(Ask::Start), request }]
    }

    fn tick(&mut self) -> Vec<Effect> {
        vec![Effect::Timer { id: self.asks.ask(Ask::Tick), ms: POLL_MS }]
    }
}

impl Screen for TvCode {
    type Event = CodeEvent;
    type View = CodeView;

    fn update(&mut self, event: CodeEvent, cx: &Context) -> Vec<Effect> {
        match event {
            CodeEvent::Opened => {
                self.asks.forget();
                *self = Self { asks: std::mem::take(&mut self.asks), ..Self::default() };
                self.start(cx)
            }
        }
    }

    fn answer(&mut self, id: u32, reply: Reply, cx: &Context) -> Vec<Effect> {
        let Some(ask) = self.asks.take(id) else { return vec![] };
        match ask {
            Ask::Start => {
                let answer = match api::outcome(reply, cx.online) {
                    Outcome::Body { body, .. } => serde_json::from_str::<Value>(&body).ok(),
                    _ => None,
                };
                let text = |o: &Value, key: &str| o.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
                match answer.filter(|o| !text(o, "code").is_empty() && !text(o, "token").is_empty()) {
                    Some(o) => {
                        let life = o.get("expires").and_then(Value::as_f64).unwrap_or(CODE_LIFE);
                        let until = cx.clock.now + life;
                        self.link = Some(Link { token: text(&o, "token"), code: text(&o, "code"), url: text(&o, "url"), until });
                        self.failed = false;
                        self.tick()
                    }
                    None => {
                        self.link = None;
                        self.failed = true;
                        vec![Effect::Timer { id: self.asks.ask(Ask::Retry), ms: RETRY_MS }]
                    }
                }
            }
            Ask::Retry => self.start(cx),
            Ask::Tick => match &self.link {
                Some(link) if cx.clock.now < link.until => {
                    let path = format!("/api/tvlink/poll?token={}", encode(&link.token));
                    vec![Effect::Request { id: self.asks.ask(Ask::Poll), request: api::get(&cx.base, &cx.who(), &path, None) }]
                }
                _ => self.start(cx),
            },
            Ask::Poll => match api::outcome(reply, cx.online) {
                Outcome::Body { body, .. }
                    if serde_json::from_str::<Value>(&body).is_ok_and(|o| o.get("ok") == Some(&Value::Bool(true))) =>
                {
                    self.signed_in = true;
                    vec![]
                }
                // (the code ran out on the server: a new one)
                Outcome::Failed { failure } if failure.status() == 410 => self.start(cx),
                _ => self.tick(),
            },
        }
    }

    fn view(&self) -> CodeView {
        CodeView {
            hint: "On your phone, open zunflix and sign in this TV, or scan the code.".into(),
            qr: self.link.as_ref().map(|l| l.url.clone()),
            code: match (&self.link, self.failed) {
                (Some(link), _) => link.code.clone(),
                (None, true) => "Couldn't reach zunflix".into(),
                (None, false) => String::new(),
            },
            spinner: self.link.is_none() && !self.failed,
            signed_in: self.signed_in,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{response, test_context, Body};

    fn sent(effects: &[Effect]) -> (u32, crate::screen::Request) {
        match effects {
            [Effect::Request { id, request }] => (*id, request.clone()),
            other => panic!("expected a request, got {other:?}"),
        }
    }

    #[test]
    fn a_phone_approves_a_code() {
        let cx = test_context(0.);
        let mut s = SignInTv::default();
        s.update(SignInEvent::Opened, &cx);
        assert!(s.update(SignInEvent::Submit { name: "".into(), code: "ab1".into() }, &cx).is_empty());
        assert_eq!(s.view().error.as_deref(), Some("The code is the 6 letters and numbers on the TV."));
        s.update(SignInEvent::Typed, &cx);
        assert_eq!(s.view().error, None);
        let (id, request) = sent(&s.update(SignInEvent::Submit { name: "  Living room ".into(), code: "ab-c12d".into() }, &cx));
        assert!(request.url.ends_with("/api/tvlink/approve"));
        assert_eq!(request.body, Some(Body::Text { text: r#"{"code":"ABC12D","name":"Living room"}"#.into(), content_type: "application/json".into() }));
        assert_eq!(s.view().button, "Signing in…");
        assert!(s.update(SignInEvent::Submit { name: "".into(), code: "ABC12D".into() }, &cx).is_empty());
        s.answer(id, response(404, r#"{"error": "That code has expired. Use the new one the TV shows now."}"#, None), &cx);
        assert_eq!(s.view().error.as_deref(), Some("That code has expired. Use the new one the TV shows now."));
        let (id, request) = sent(&s.update(SignInEvent::Submit { name: "".into(), code: "ABC12D".into() }, &cx));
        assert!(matches!(request.body, Some(Body::Text { ref text, .. }) if text == r#"{"code":"ABC12D"}"#));
        s.answer(id, response(200, r#"{"ok": true}"#, None), &cx);
        let view = s.view();
        assert_eq!((view.stage, view.title.as_str()), (Stage::Done, "Your TV is signing in"));
    }

    #[test]
    fn the_steps_and_back() {
        let cx = test_context(0.);
        let mut s = SignInTv::default();
        s.update(SignInEvent::Steps, &cx);
        let view = s.view();
        assert_eq!(view.stage, Stage::Steps);
        assert_eq!(view.steps[1].bold, "host.invalid/get");
        s.update(SignInEvent::Back, &cx);
        assert_eq!(s.view().stage, Stage::Code);
    }

    #[test]
    fn the_tv_shows_a_code_and_waits_for_a_phone() {
        let mut cx = test_context(1000.);
        let mut tv = TvCode::default();
        let (start, request) = sent(&tv.update(CodeEvent::Opened, &cx));
        assert!(request.url.ends_with("/api/tvlink/start"));
        assert!(tv.view().spinner);
        let answer = r#"{"code": "ABC123", "token": "t/1", "url": "https://host.invalid/link?code=ABC123", "expires": 600}"#;
        let effects = tv.answer(start, response(200, answer, None), &cx);
        let [Effect::Timer { id: tick, ms: POLL_MS }] = effects[..] else { panic!("{effects:?}") };
        let view = tv.view();
        assert_eq!((view.code.as_str(), view.qr.as_deref()), ("ABC123", Some("https://host.invalid/link?code=ABC123")));
        let (poll, request) = sent(&tv.answer(tick, Reply::Fired, &cx));
        assert!(request.url.ends_with("/api/tvlink/poll?token=t%2F1"));
        assert!(request.cookies);
        let [Effect::Timer { id: tick, .. }] = tv.answer(poll, response(200, r#"{"waiting": true}"#, None), &cx)[..] else { panic!() };
        let (poll, _) = sent(&tv.answer(tick, Reply::Fired, &cx));
        assert!(tv.answer(poll, response(200, r#"{"ok": true}"#, None), &cx).is_empty());
        assert!(tv.view().signed_in);
        // (ten minutes on, a new code)
        let mut tv = TvCode::default();
        let (start, _) = sent(&tv.update(CodeEvent::Opened, &cx));
        let [Effect::Timer { id: tick, .. }] = tv.answer(start, response(200, answer, None), &cx)[..] else { panic!() };
        cx.clock.now += 601.;
        let (_, again) = sent(&tv.answer(tick, Reply::Fired, &cx));
        assert!(again.url.ends_with("/api/tvlink/start"));
    }

    #[test]
    fn no_code_says_so_and_asks_again() {
        let cx = test_context(0.);
        let mut tv = TvCode::default();
        let (start, _) = sent(&tv.update(CodeEvent::Opened, &cx));
        let effects = tv.answer(start, Reply::Unreachable { offline: false, message: "".into() }, &cx);
        let [Effect::Timer { id, ms: RETRY_MS }] = effects[..] else { panic!("{effects:?}") };
        let view = tv.view();
        assert_eq!((view.code.as_str(), view.spinner), ("Couldn't reach zunflix", false));
        assert!(tv.answer(id, Reply::Fired, &cx).len() == 1);
    }
}
