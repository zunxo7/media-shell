//! How long apps keep what the server sent, and when they ask again: one table for every app. The apps keep the pages
//! (memory, and their own folder on disk) and make the requests; the core decides.
//!
//! A kept page is used as it is while fresh. Past that it's still shown while the app asks again (with its ETag, so
//! an unchanged page comes back as a bodiless 304), up to its `usable` age; offline, any age is better than nothing.

/// How long a kind of page is fresh, and how long it may still be shown while a newer one is fetched (seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    pub fresh: f64,
    pub usable: f64,
}

const DAY: f64 = 86_400.;

/// The rule for a server path; `fresh` and `usable` 0 for a page never kept.
pub fn rule(path: &str) -> Rule {
    let path = path.split('?').next().unwrap_or(path);
    let (fresh, usable) = match path {
        // (what's on Home changes through the day: a newer one is asked for behind it every time it's shown)
        "/api/home" => (5., 300.),
        "/api/progress" | "/api/up-next" => (60., DAY),
        "/api/anime" => (DAY, 7. * DAY),
        "/api/cinema" => (600., DAY),
        p if p.starts_with("/api/title/") => (600., 7. * DAY),
        // (a shelf's first page shows at once while a newer one comes; a series' chapters, offline, for good)
        "/api/manhwa" => (300., DAY),
        "/api/manhwa/chapters" => (300., 30. * DAY),
        // (a chapter's pages don't change once it's out: a chapter read opens again with no connection)
        "/api/manhwa/chapter" => (DAY, 30. * DAY),
        _ => (0., 0.),
    };
    Rule { fresh, usable }
}

/// A page an app kept: its body, the server's ETag for it, and when it came (seconds since 1970).
#[derive(Clone, Debug, PartialEq)]
pub struct Kept {
    pub body: String,
    pub etag: Option<String>,
    pub at: f64,
}

/// What to do about a page.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// The kept page is fresh: use it, ask nothing.
    Use,
    /// Ask the server, sending `etag` (a 304 keeps the kept page). `show`: show the kept page meanwhile.
    Refresh { show: bool, etag: Option<String> },
    /// Nothing usable is kept: ask, and wait for the answer.
    Fetch,
}

/// What to do about `path`, with what's `kept` of it, at `now` (seconds since 1970).
pub fn plan(path: &str, kept: Option<&Kept>, now: f64) -> Plan {
    let rule = rule(path);
    let Some(kept) = kept.filter(|_| rule.usable > 0.) else {
        return Plan::Fetch;
    };
    let age = now - kept.at;
    if age < rule.fresh {
        Plan::Use
    } else {
        Plan::Refresh { show: age < rule.usable, etag: kept.etag.clone() }
    }
}

/// Pages kept on disk, at most (the oldest go first).
pub const DISK_CAP: u64 = 20 * 1024 * 1024;

/// The pages to delete so what's kept fits in `cap` bytes: the oldest first. `pages`: (name, size, when it came).
pub fn evict(pages: &[(String, u64, f64)], cap: u64) -> Vec<String> {
    let mut by_age: Vec<_> = pages.iter().collect();
    by_age.sort_by(|a, b| a.2.total_cmp(&b.2));
    let mut total: u64 = pages.iter().map(|p| p.1).sum();
    let mut gone = Vec::new();
    for (name, size, _) in by_age {
        if total <= cap {
            break;
        }
        total -= size;
        gone.push(name.clone());
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(at: f64) -> Kept {
        Kept { body: "{}".into(), etag: Some("\"a\"".into()), at }
    }

    #[test]
    fn rules_by_path() {
        assert_eq!(rule("/api/home"), Rule { fresh: 5., usable: 300. });
        assert_eq!(rule("/api/title/cinema-movie-tt1234567"), Rule { fresh: 600., usable: 7. * DAY });
        assert_eq!(rule("/api/cinema?type=movie&skip=0").fresh, 600.);
        assert_eq!(rule("/api/timing"), Rule { fresh: 0., usable: 0. });
        assert_eq!(rule("/api/manhwa?page=1&q=&genre=&sort=popular"), Rule { fresh: 300., usable: DAY });
        assert_eq!(rule("/api/manhwa/chapter?path=x"), Rule { fresh: DAY, usable: 30. * DAY });
    }

    #[test]
    fn fresh_is_used_stale_is_shown_while_asked_old_is_fetched() {
        assert_eq!(plan("/api/home", Some(&kept(1000.)), 1003.), Plan::Use);
        assert_eq!(
            plan("/api/home", Some(&kept(1000.)), 1100.),
            Plan::Refresh { show: true, etag: Some("\"a\"".into()) }
        );
        assert_eq!(
            plan("/api/home", Some(&kept(1000.)), 2000.),
            Plan::Refresh { show: false, etag: Some("\"a\"".into()) }
        );
        assert_eq!(plan("/api/home", None, 2000.), Plan::Fetch);
        assert_eq!(plan("/api/timing", Some(&kept(1000.)), 1001.), Plan::Fetch);
    }

    #[test]
    fn oldest_go_first_until_it_fits() {
        let pages = vec![("new".into(), 10, 30.), ("old".into(), 10, 10.), ("mid".into(), 10, 20.)];
        assert_eq!(evict(&pages, 15), vec!["old".to_string(), "mid".to_string()]);
        assert!(evict(&pages, 30).is_empty());
    }
}
