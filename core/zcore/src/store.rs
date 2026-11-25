//! The app's one store of server pages, shared by every screen (and by the app's own code until its screens move):
//! each page is read from the device once, asked of the server once at a time however many want it, kept in memory,
//! and told about when it changes. By [`crate::cache`]'s rules: a fresh copy is answered at once, a stale one is shown
//! while the server is asked again behind it, one too old is waited on, and with no answer a kept copy of any age
//! beats an error. The pages stay kept on the device as they always were (`Load`/`Store`).

use crate::api::{self, Failure, Kind, Outcome};
use crate::cache::{self, Kept, Plan};
use crate::screen::{Context, Reply};
use std::collections::HashMap;

/// What copy of a page will do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// Whatever the rules allow: a stale copy at once, with the server asked behind it.
    Any,
    /// A current copy: the kept one while fresh, else the server's (a pass that already showed the kept one).
    Current,
    /// Straight from the server, however fresh the kept copy (Try again, something just changed it).
    Server,
}

/// A page as a waiter gets it.
#[derive(Clone, Debug, PartialEq)]
pub struct PageReply {
    /// The page, None when there's none to show.
    pub body: Option<String>,
    /// When the body came from the server (seconds since 1970).
    pub at: f64,
    /// Why the server's newest copy didn't come, when it didn't.
    pub failure: Option<Failure>,
    /// The body is a stale copy, and the server is being asked again behind it: a newer one is told about as changed.
    pub stale: bool,
}

/// What the store needs done, for the root to carry out under its own ids.
#[derive(Clone, Debug, PartialEq)]
pub enum Io {
    /// Read the page kept under `key`.
    Load { key: String },
    /// Ask the server for the page at `path`; `etag`, the kept copy's.
    Ask { path: String, etag: Option<String> },
    /// Keep the page on the device.
    Keep { key: String, body: String, etag: Option<String> },
}

/// Which of a page's steps an answer is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Load,
    Ask,
}

/// What a call to the store asks of the root: I/O to do (by page key, with the step its answer is for), pages to hand
/// to their waiters, the keys of the pages whose body changed, and whether the server signed the app out.
#[derive(Debug, PartialEq)]
pub struct Out<W> {
    pub io: Vec<(String, Step, Io)>,
    pub give: Vec<(W, PageReply)>,
    pub changed: Vec<String>,
    pub signed_out: bool,
}

impl<W> Default for Out<W> {
    fn default() -> Self {
        Self { io: vec![], give: vec![], changed: vec![], signed_out: false }
    }
}

#[derive(Debug)]
struct Page<W> {
    path: String,
    kept: Option<Kept>,
    /// The device's copy has been read (or there was none).
    read: bool,
    loading: bool,
    asking: bool,
    /// Waiting for the server (or the device), and what copy will do for each.
    waiting: Vec<(W, Need)>,
}

/// The store. See the module's docs. Pages are known by the key they're kept under, as one path can be several pages
/// (each profile's progress).
#[derive(Debug)]
pub struct Store<W> {
    pages: HashMap<String, Page<W>>,
}

impl<W> Default for Store<W> {
    fn default() -> Self {
        Self { pages: HashMap::new() }
    }
}

impl<W: Clone> Store<W> {
    /// `waiter` wants the page at `path`, kept under `key`, as `need` says.
    pub fn want(&mut self, path: &str, key: &str, waiter: W, need: Need, cx: &Context) -> Out<W> {
        let mut out = Out::default();
        let page = self.pages.entry(key.into()).or_insert_with(|| Page {
            path: path.into(),
            kept: None,
            read: false,
            loading: false,
            asking: false,
            waiting: vec![],
        });
        if !page.read {
            page.waiting.push((waiter, need));
            if !page.loading {
                page.loading = true;
                out.io.push((key.into(), Step::Load, Io::Load { key: key.into() }));
            }
            return out;
        }
        serve(key, page, waiter, need, cx, &mut out);
        out
    }

    /// The answer to the page `key`'s step.
    pub fn answer(&mut self, key: &str, step: Step, reply: Reply, cx: &Context) -> Out<W> {
        let mut out = Out::default();
        let Some(page) = self.pages.get_mut(key) else { return out };
        match step {
            Step::Load => {
                page.loading = false;
                page.read = true;
                if let Reply::Kept { body, etag, at } = reply {
                    page.kept = Some(Kept { body, etag, at });
                }
                for (waiter, need) in std::mem::take(&mut page.waiting) {
                    serve(key, page, waiter, need, cx, &mut out);
                }
            }
            Step::Ask => {
                page.asking = false;
                let waiting = std::mem::take(&mut page.waiting);
                match api::outcome(reply, cx.online) {
                    Outcome::Body { body, etag } => {
                        if page.kept.as_ref().is_some_and(|k| k.body != body) {
                            out.changed.push(key.into());
                        }
                        page.kept = Some(Kept { body: body.clone(), etag: etag.clone(), at: cx.clock.now });
                        out.io.push((key.into(), Step::Ask, Io::Keep { key: key.into(), body, etag }));
                    }
                    Outcome::Unchanged => match page.kept.as_mut() {
                        Some(kept) => {
                            kept.at = cx.clock.now;
                            let (body, etag) = (kept.body.clone(), kept.etag.clone());
                            out.io.push((key.into(), Step::Ask, Io::Keep { key: key.into(), body, etag }));
                        }
                        // (a 304 with nothing kept to stand for: asked again whole)
                        None => {
                            page.waiting = waiting;
                            ask(key, page, &mut out);
                            return out;
                        }
                    },
                    Outcome::Failed { failure } => {
                        out.signed_out |= failure.kind == Kind::SignedOut;
                        let reply = PageReply {
                            body: page.kept.as_ref().map(|k| k.body.clone()),
                            at: page.kept.as_ref().map_or(0., |k| k.at),
                            failure: Some(failure),
                            stale: false,
                        };
                        out.give.extend(waiting.into_iter().map(|(w, _)| (w, reply.clone())));
                        return out;
                    }
                }
                let reply = current(page, false);
                out.give.extend(waiting.into_iter().map(|(w, _)| (w, reply.clone())));
            }
        }
        out
    }

    /// Every page forgotten from memory (signed out): each is read again when next wanted.
    pub fn forget(&mut self) {
        self.pages.clear();
    }

    /// One page forgotten from memory (another profile's Home, a list the server was still putting together); one
    /// being asked for is left to finish.
    pub fn forget_one(&mut self, key: &str) {
        if self.pages.get(key).is_some_and(|p| !p.loading && !p.asking) {
            self.pages.remove(key);
        }
    }
}

/// The page as kept now.
fn current<W>(page: &Page<W>, stale: bool) -> PageReply {
    PageReply {
        body: page.kept.as_ref().map(|k| k.body.clone()),
        at: page.kept.as_ref().map_or(0., |k| k.at),
        failure: None,
        stale,
    }
}

/// A page already read from the device, by the cache rules.
fn serve<W: Clone>(key: &str, page: &mut Page<W>, waiter: W, need: Need, cx: &Context, out: &mut Out<W>) {
    let plan = cache::plan(&page.path, page.kept.as_ref(), cx.clock.now);
    match plan {
        Plan::Use if need != Need::Server => out.give.push((waiter, current(page, false))),
        Plan::Refresh { show: true, .. } if need == Need::Any => {
            out.give.push((waiter, current(page, true)));
            ask(key, page, out);
        }
        _ => {
            page.waiting.push((waiter, need));
            ask(key, page, out);
        }
    }
}

/// The server asked for the page, unless it's being asked already.
fn ask<W>(key: &str, page: &mut Page<W>, out: &mut Out<W>) {
    if page.asking {
        return;
    }
    page.asking = true;
    let etag = page.kept.as_ref().and_then(|k| k.etag.clone());
    out.io.push((key.into(), Step::Ask, Io::Ask { path: page.path.clone(), etag }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{response, test_context};

    const PATH: &str = "/api/progress"; // (fresh a minute, usable a day)

    fn kept(at: f64) -> Reply {
        Reply::Kept { body: "old".into(), etag: Some("e1".into()), at }
    }

    fn reply(body: &str, at: f64, stale: bool) -> PageReply {
        PageReply { body: Some(body.into()), at, failure: None, stale }
    }

    #[test]
    fn many_waiters_share_one_read_and_one_request() {
        let cx = test_context(1000.);
        let mut store = Store::default();
        let first = store.want(PATH, "k", 1, Need::Any, &cx);
        assert_eq!(first.io, [("k".into(), Step::Load, Io::Load { key: "k".into() })]);
        assert!(store.want(PATH, "k", 2, Need::Any, &cx).io.is_empty());
        let asked = store.answer("k", Step::Load, Reply::Missing, &cx);
        assert_eq!(asked.io, [("k".into(), Step::Ask, Io::Ask { path: PATH.into(), etag: None })]);
        assert!(store.want(PATH, "k", 3, Need::Any, &cx).io.is_empty());
        let came = store.answer("k", Step::Ask, response(200, "new", Some("e2")), &cx);
        let given: Vec<i32> = came.give.iter().map(|g| g.0).collect();
        assert_eq!(given, [1, 2, 3]);
        assert_eq!(came.give[0].1, reply("new", 1000., false));
        assert_eq!(came.io, [("k".into(), Step::Ask, Io::Keep { key: "k".into(), body: "new".into(), etag: Some("e2".into()) })]);
        // (in memory now: the next waiter is answered at once)
        let again = store.want(PATH, "k", 4, Need::Any, &cx);
        assert!(again.io.is_empty());
        assert_eq!(again.give, [(4, reply("new", 1000., false))]);
    }

    #[test]
    fn a_stale_copy_shows_at_once_and_a_newer_one_is_told_as_changed() {
        let cx = test_context(1000.);
        let mut store = Store::default();
        store.want(PATH, "k", 1, Need::Any, &cx);
        let out = store.answer("k", Step::Load, kept(500.), &cx);
        assert_eq!(out.give, [(1, reply("old", 500., true))]);
        assert_eq!(out.io, [("k".into(), Step::Ask, Io::Ask { path: PATH.into(), etag: Some("e1".into()) })]);
        let came = store.answer("k", Step::Ask, response(200, "new", None), &cx);
        assert_eq!(came.changed, ["k".to_string()]);
        assert!(came.give.is_empty());
    }

    #[test]
    fn a_304_keeps_the_copy_and_makes_it_fresh() {
        let cx = test_context(1000.);
        let mut store = Store::default();
        store.want(PATH, "k", 1, Need::Server, &cx);
        store.answer("k", Step::Load, kept(990.), &cx);
        let out = store.answer("k", Step::Ask, response(304, "", None), &cx);
        assert_eq!(out.give, [(1, reply("old", 1000., false))]);
        assert!(out.changed.is_empty());
        assert_eq!(out.io, [("k".into(), Step::Ask, Io::Keep { key: "k".into(), body: "old".into(), etag: Some("e1".into()) })]);
    }

    #[test]
    fn too_old_is_waited_on_but_beats_an_error() {
        let cx = test_context(1_000_000.);
        let mut store = Store::default();
        store.want(PATH, "k", 1, Need::Any, &cx);
        let out = store.answer("k", Step::Load, kept(1.), &cx);
        assert!(out.give.is_empty());
        let failed = store.answer("k", Step::Ask, Reply::Unreachable { offline: true, message: String::new() }, &cx);
        let (_, page) = &failed.give[0];
        assert_eq!(page.body.as_deref(), Some("old"));
        assert_eq!(page.failure.as_ref().map(|f| f.kind), Some(Kind::Offline));
    }

    #[test]
    fn nothing_and_no_answer_is_a_failure_and_a_401_signs_out() {
        let cx = test_context(1000.);
        let mut store = Store::default();
        store.want(PATH, "k", 1, Need::Any, &cx);
        store.answer("k", Step::Load, Reply::Missing, &cx);
        let out = store.answer("k", Step::Ask, response(401, "", None), &cx);
        assert!(out.signed_out);
        assert_eq!(out.give[0].1.body, None);
        assert_eq!(out.give[0].1.failure.as_ref().map(|f| f.kind), Some(Kind::SignedOut));
    }

    #[test]
    fn fresh_asks_the_server_even_when_the_copy_is_fresh() {
        let cx = test_context(1000.);
        let mut store = Store::default();
        store.want(PATH, "k", 1, Need::Any, &cx);
        store.answer("k", Step::Load, kept(999.), &cx);
        let out = store.want(PATH, "k", 2, Need::Server, &cx);
        assert_eq!(out.io.len(), 1);
        assert!(out.give.is_empty());
    }

    #[test]
    fn current_waits_past_a_stale_copy_but_takes_a_fresh_one() {
        let cx = test_context(1000.);
        let mut store = Store::default();
        store.want(PATH, "k", 1, Need::Current, &cx);
        let out = store.answer("k", Step::Load, kept(500.), &cx);
        assert!(out.give.is_empty());
        assert_eq!(out.io.len(), 1);
        let came = store.answer("k", Step::Ask, response(200, "new", None), &cx);
        assert_eq!(came.give, [(1, reply("new", 1000., false))]);
        assert_eq!(store.want(PATH, "k", 2, Need::Current, &cx).give, [(2, reply("new", 1000., false))]);
    }

    #[test]
    fn forgetting_reads_the_device_again() {
        let cx = test_context(1000.);
        let mut store = Store::default();
        store.want(PATH, "k", 1, Need::Any, &cx);
        store.answer("k", Step::Load, kept(999.), &cx);
        store.forget();
        assert_eq!(store.want(PATH, "k", 2, Need::Any, &cx).io, [("k".into(), Step::Load, Io::Load { key: "k".into() })]);
    }
}
