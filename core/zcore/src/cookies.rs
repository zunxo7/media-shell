//! The sign-in's cookies: which a server's answer sets, which go back on a request, which are kept on the device.
//!
//! The core does no I/O: the app gives it each answer's `Set-Cookie` lines and the address a request goes to, and keeps
//! the text [`Jar::save`] gives. Both apps use these rules, so they agree on which cookies to keep. The saved form is
//! the one the apps always kept (`[{"n","v","e","p","d","h","s","o"}]`): changing it signs everybody out.

use serde_json::{json, Value};

/// A cookie that ends when the app does (no Max-Age, no Expires): never kept on disk.
const SESSION: i64 = i64::MAX;

#[derive(Clone, Debug, PartialEq)]
struct Cookie {
    name: String,
    value: String,
    /// When it expires, milliseconds since 1970.
    expires: i64,
    path: String,
    domain: String,
    /// Only for the host that set it (no Domain attribute).
    host_only: bool,
    secure: bool,
    http_only: bool,
    /// Kept on the device (it has an expiry).
    keep: bool,
}

/// The cookies held, in order they were set.
#[derive(Clone, Debug, Default)]
pub struct Jar {
    cookies: Vec<Cookie>,
}

impl Jar {
    /// The jar a saved text describes; what can't be read is an empty jar.
    pub fn load(saved: &str) -> Jar {
        let Ok(Value::Array(list)) = serde_json::from_str::<Value>(saved) else { return Jar::default() };
        let text = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let mut cookies: Vec<Cookie> = Vec::new();
        for o in list {
            let cookie = Cookie {
                name: text(&o, "n"),
                value: text(&o, "v"),
                expires: o.get("e").and_then(|e| e.as_i64().or_else(|| e.as_f64().map(|f| f as i64))).unwrap_or(0),
                path: text(&o, "p"),
                domain: text(&o, "d"),
                host_only: o.get("h").and_then(Value::as_bool).unwrap_or(true),
                secure: o.get("s").and_then(Value::as_bool).unwrap_or(false),
                http_only: o.get("o").and_then(Value::as_bool).unwrap_or(false),
                keep: true,
            };
            cookies.retain(|x| !same(x, &cookie));
            cookies.push(cookie);
        }
        Jar { cookies }
    }

    /// What to keep on the device: the cookies that have an expiry.
    pub fn save(&self) -> String {
        let list: Vec<Value> = self
            .cookies
            .iter()
            .filter(|c| c.keep)
            .map(|c| json!({"n": c.name, "v": c.value, "e": c.expires, "p": c.path, "d": c.domain, "h": c.host_only, "s": c.secure, "o": c.http_only}))
            .collect();
        Value::Array(list).to_string()
    }

    /// Takes the `Set-Cookie` lines of an answer from `url`. True when the jar changed (keep [`Jar::save`]'s text).
    pub fn take(&mut self, url: &str, set_cookies: &[String], now_ms: i64) -> bool {
        let Some((host, _, _)) = parts(url) else { return false };
        let mut changed = false;
        for line in set_cookies {
            let Some(cookie) = parse(line, &host, now_ms) else { continue };
            self.cookies.retain(|x| !same(x, &cookie));
            if cookie.expires > now_ms {
                self.cookies.push(cookie);
            }
            changed = true;
        }
        changed
    }

    /// The `Cookie` header a request to `url` carries, if any cookie goes.
    pub fn header(&self, url: &str, now_ms: i64) -> Option<String> {
        let (host, secure, path) = parts(url)?;
        let all: Vec<String> = self
            .cookies
            .iter()
            .filter(|c| c.expires > now_ms && (!c.secure || secure) && path_matches(&path, &c.path) && domain_matches(&host, c))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        (!all.is_empty()).then(|| all.join("; "))
    }

    /// Forgets every cookie (signing out).
    pub fn clear(&mut self) {
        self.cookies.clear();
    }
}

fn same(a: &Cookie, b: &Cookie) -> bool {
    a.name == b.name && a.domain == b.domain && a.path == b.path
}

/// An address's host, whether it is https, and its path.
fn parts(url: &str) -> Option<(String, bool, String)> {
    let url = url::Url::parse(url).ok()?;
    Some((url.host_str()?.to_ascii_lowercase(), url.scheme() == "https", url.path().to_string()))
}

fn path_matches(path: &str, cookie: &str) -> bool {
    path == cookie || path.strip_prefix(cookie).is_some_and(|rest| cookie.ends_with('/') || rest.starts_with('/'))
}

fn domain_matches(host: &str, c: &Cookie) -> bool {
    host == c.domain || (!c.host_only && host.ends_with(&format!(".{}", c.domain)))
}

/// One `Set-Cookie` line from a server at `host`; None when it's malformed or names a domain the host isn't in.
fn parse(line: &str, host: &str, now_ms: i64) -> Option<Cookie> {
    let mut attributes = line.split(';');
    let (name, value) = attributes.next()?.split_once('=')?;
    let mut c = Cookie {
        name: name.trim().into(),
        value: value.trim().into(),
        expires: SESSION,
        path: "/".into(),
        domain: host.into(),
        host_only: true,
        secure: false,
        http_only: false,
        keep: false,
    };
    let mut max_age = false;
    for a in attributes {
        let (k, v) = a.split_once('=').unwrap_or((a, ""));
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "max-age" => {
                if let Ok(secs) = v.parse::<i64>() {
                    c.expires = now_ms.saturating_add(secs.saturating_mul(1000));
                    c.keep = true;
                    max_age = true;
                }
            }
            "expires" if !max_age => {
                if let Some(t) = http_date(v) {
                    c.expires = t;
                    c.keep = true;
                }
            }
            "path" => c.path = v.into(),
            "domain" => {
                c.domain = v.trim_start_matches('.').to_ascii_lowercase();
                c.host_only = false;
            }
            "secure" => c.secure = true,
            "httponly" => c.http_only = true,
            _ => {}
        }
    }
    let inside = c.domain == host || host.strip_suffix(&c.domain).is_some_and(|prefix| prefix.ends_with('.'));
    inside.then_some(c)
}

/// "Wed, 21 Oct 2026 07:28:00 GMT" to milliseconds.
fn http_date(s: &str) -> Option<i64> {
    let p: Vec<&str> = s.split([' ', '-', ':', ',']).filter(|x| !x.is_empty()).collect();
    if p.len() < 7 {
        return None;
    }
    let day: i64 = p[1].parse().ok()?;
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].iter().position(|m| p[2].starts_with(m))? as i64 + 1;
    let mut year: i64 = p[3].parse().ok()?;
    if year < 100 {
        year += 2000;
    }
    let (h, m, sec): (i64, i64, i64) = (p[4].parse().ok()?, p[5].parse().ok()?, p[6].parse().ok()?);
    let (y, mo) = if month <= 2 { (year - 1, month + 9) } else { (year, month - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * mo + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(((era * 146097 + doe - 719468) * 86400 + h * 3600 + m * 60 + sec) * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const URL: &str = "https://host.invalid/api/sign-in";

    fn lines(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_cookie_comes_back_on_the_next_request() {
        let mut jar = Jar::default();
        assert!(jar.take(URL, &lines(&["zf_a=abc; Path=/; Max-Age=60; HttpOnly"]), NOW));
        assert_eq!(jar.header("https://host.invalid/api/home", NOW).as_deref(), Some("zf_a=abc"));
        assert_eq!(jar.header("https://host.invalid/api/home", NOW + 61_000), None);
        assert_eq!(jar.header("https://other.com/", NOW), None);
    }

    #[test]
    fn only_cookies_with_an_expiry_are_kept_on_disk() {
        let mut jar = Jar::default();
        jar.take(URL, &lines(&["a=1; Max-Age=60", "b=2", "c=3; Expires=Wed, 21 Oct 2037 07:28:00 GMT"]), NOW);
        let saved = Jar::load(&jar.save());
        assert_eq!(saved.header(URL, NOW).as_deref(), Some("a=1; c=3"));
        assert_eq!(jar.header(URL, NOW).as_deref(), Some("a=1; b=2; c=3"));
    }

    #[test]
    fn max_age_beats_expires_and_zero_removes() {
        let mut jar = Jar::default();
        jar.take(URL, &lines(&["a=1; Max-Age=60; Expires=Wed, 21 Oct 2015 07:28:00 GMT"]), NOW);
        assert!(jar.header(URL, NOW).is_some());
        assert!(jar.take(URL, &lines(&["a=; Max-Age=0"]), NOW));
        assert_eq!(jar.header(URL, NOW), None);
    }

    #[test]
    fn domain_path_and_secure_rules() {
        let mut jar = Jar::default();
        jar.take("https://api.host.invalid/x", &lines(&["d=1; Domain=.host.invalid", "p=1; Path=/api", "s=1; Secure", "bad=1; Domain=evil.com"]), NOW);
        assert_eq!(jar.header("https://www.host.invalid/", NOW).as_deref(), Some("d=1"));
        assert_eq!(jar.header("https://api.host.invalid/api/x", NOW).as_deref(), Some("d=1; p=1; s=1"));
        assert_eq!(jar.header("https://api.host.invalid/apix", NOW).as_deref(), Some("d=1; s=1"));
        assert_eq!(jar.header("http://api.host.invalid/api", NOW).as_deref(), Some("d=1; p=1"));
        assert_eq!(jar.header("https://evil.com/", NOW), None);
    }

    #[test]
    fn the_saved_form_the_apps_always_kept_still_loads() {
        let old = r#"[{"n":"zf_a","v":"tok","e":4102444800000,"p":"/","d":"host.invalid","h":true,"s":false,"o":true}]"#;
        let jar = Jar::load(old);
        assert_eq!(jar.header(URL, NOW).as_deref(), Some("zf_a=tok"));
        assert_eq!(Jar::load(&jar.save()).save(), jar.save());
        assert_eq!(Jar::load("nonsense").save(), "[]");
    }

    #[test]
    fn clear_forgets_everything() {
        let mut jar = Jar::default();
        jar.take(URL, &lines(&["a=1; Max-Age=60"]), NOW);
        jar.clear();
        assert_eq!(jar.save(), "[]");
    }
}
