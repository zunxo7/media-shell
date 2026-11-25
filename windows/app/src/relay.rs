//! A local relay for remote video files (TorBox's): mpv opens a file in three trips one after another (its start,
//! its index and tags at the end, back to the start), each a fresh HTTPS request at ~1 s from here. The relay fetches
//! the start and the end at once as soon as a file is known (from a title page's warm-up, or at Play) and answers
//! those trips from memory; for a resume, also the stretch it lands on (found in an MKV's index, its Cues). Anything
//! else it passes through, on a connection kept open (~0.2 s to start, against ~1.2 s on a new one).

use futures_util::StreamExt;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

/// The start kept: its header, and the first seconds of the film.
const HEAD: u64 = 8 * 1024 * 1024;
/// The end kept: an MKV's index (Cues) and tags.
const TAIL: u64 = 4 * 1024 * 1024;
/// Files kept at once (a title page's warm-up, the next episode, what plays).
const KEPT: usize = 4;
/// Read ahead where a resume lands: the seconds after it.
const MID: u64 = 12 * 1024 * 1024;
/// A resume nearer the start than this plays from the kept start.
const MID_AFTER: f64 = 30.0;

/// A stretch of a file coming in from `at`, readable while it's still arriving.
struct Part {
    at: u64,
    /// How much was asked for.
    len: u64,
    bytes: Mutex<Vec<u8>>,
    done: Mutex<bool>,
    more: Notify,
}

impl Part {
    fn new(at: u64, len: u64) -> Arc<Part> {
        Arc::new(Part { at, len, bytes: Mutex::new(Vec::new()), done: Mutex::new(false), more: Notify::new() })
    }
    fn covers(&self, at: u64) -> bool {
        at >= self.at && at < self.at + self.len
    }
    fn closed(&self) {
        *self.done.lock().unwrap() = true;
        self.more.notify_waiters();
    }
    fn end(&self) -> u64 {
        self.at + self.bytes.lock().unwrap().len() as u64
    }
    fn finished(&self) -> bool {
        *self.done.lock().unwrap()
    }
}

struct File {
    url: String,
    agent: String,
    /// The whole size, once the start's answer has said it.
    size: Mutex<Option<u64>>,
    sized: Notify,
    head: Arc<Part>,
    tail: Mutex<Option<Arc<Part>>>,
    /// Where a resume lands, and the seconds it was for.
    mid: Mutex<Option<(f64, Arc<Part>)>>,
}

struct Relay {
    port: u16,
    files: Mutex<Vec<(String, Arc<File>)>>,
}

fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().connect_timeout(std::time::Duration::from_millis(zcore::api::transport().connect_ms.into())).build().unwrap())
}

fn relay() -> Option<&'static Relay> {
    static R: OnceLock<Option<Relay>> = OnceLock::new();
    R.get_or_init(|| {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
        std_listener.set_nonblocking(true).ok()?;
        let port = std_listener.local_addr().ok()?.port();
        crate::spawn(async move {
            let Ok(listener) = TcpListener::from_std(std_listener) else { return };
            while let Ok((socket, _)) = listener.accept().await {
                crate::spawn(async move {
                    let _ = serve(socket).await;
                });
            }
        });
        Some(Relay { port, files: Mutex::new(Vec::new()) })
    })
    .as_ref()
}

/// The relay's address for `url`, its start and end being fetched from now, and where `start` (seconds) lands
/// (None: the relay couldn't start).
pub fn local(url: &str, agent: &str, start: f64) -> Option<String> {
    let r = relay()?;
    let id = {
        let mut files = r.files.lock().unwrap();
        match files.iter().position(|(_, f)| f.url == url) {
            Some(i) => {
                fetch_mid(files[i].1.clone(), start);
                files[i].0.clone()
            }
            None => {
                let id = format!("{:016x}", rand_id());
                let file = Arc::new(File {
                    url: url.into(),
                    agent: agent.into(),
                    size: Mutex::new(None),
                    sized: Notify::new(),
                    head: Part::new(0, HEAD),
                    tail: Mutex::new(None),
                    mid: Mutex::new(None),
                });
                files.push((id.clone(), file.clone()));
                while files.len() > KEPT {
                    files.remove(0);
                }
                fetch_start_and_end(file.clone());
                fetch_mid(file, start);
                id
            }
        }
    };
    Some(format!("http://127.0.0.1:{}/f/{id}", r.port))
}

/// A file no longer wanted.
pub fn drop_url(url: &str) {
    if let Some(r) = relay() {
        r.files.lock().unwrap().retain(|(_, f)| f.url != url);
    }
}

fn rand_id() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.finish()
}

/// Its start asked for, and its end as soon as the start's answer gives the size (asked by position: Real-Debrid
/// answers a "last N bytes" request with the first N).
fn fetch_start_and_end(file: Arc<File>) {
    let f = file.clone();
    crate::spawn(async move {
        let head = client().get(&f.url).header("User-Agent", &f.agent).header("Range", format!("bytes=0-{}", HEAD - 1)).send().await;
        let Ok(head) = head else {
            f.head.closed();
            f.sized.notify_waiters();
            return;
        };
        if let Some(size) = whole(&head) {
            *f.size.lock().unwrap() = Some(size);
            let at = size.saturating_sub(TAIL);
            let part = Part::new(at, size - at);
            *f.tail.lock().unwrap() = Some(part.clone());
            let (url, agent) = (f.url.clone(), f.agent.clone());
            crate::spawn(async move {
                match client().get(&url).header("User-Agent", &agent).header("Range", format!("bytes={at}-{}", size - 1)).send().await {
                    Ok(tail) if tail.headers().get("content-range").and_then(|v| v.to_str().ok()).is_some_and(|r| r.starts_with(&format!("bytes {at}-"))) => {
                        fill(part, tail).await;
                    }
                    // (not the stretch asked for: nothing kept, those reads go to the source)
                    _ => part.closed(),
                }
            });
        }
        f.sized.notify_waiters();
        fill(f.head.clone(), head).await;
    });
}

/// The whole size a ranged answer says ("bytes 0-99/12345").
fn whole(r: &reqwest::Response) -> Option<u64> {
    r.headers().get("content-range")?.to_str().ok()?.rsplit_once('/')?.1.trim().parse().ok()
}

async fn fill(part: Arc<Part>, response: reqwest::Response) {
    let mut body = response.bytes_stream();
    while let Some(Ok(chunk)) = body.next().await {
        part.bytes.lock().unwrap().extend_from_slice(&chunk);
        part.more.notify_waiters();
    }
    part.closed();
}

/// A whole part, once it's in (None: it didn't come).
async fn all_of(part: &Part) -> Option<Vec<u8>> {
    loop {
        let waiting = part.more.notified();
        if part.finished() {
            let bytes = part.bytes.lock().unwrap();
            return (!bytes.is_empty()).then(|| bytes.clone());
        }
        waiting.await;
    }
}

/// For a resume: once the start and end are in, the stretch `start` lands on, from the MKV's Cues (from one cue
/// earlier, as mpv may step back to it). Nothing when it isn't an MKV with its Cues in the kept end.
fn fetch_mid(file: Arc<File>, start: f64) {
    if start < MID_AFTER || file.mid.lock().unwrap().as_ref().is_some_and(|(at, _)| (at - start).abs() < 1.0) {
        return;
    }
    crate::spawn(async move {
        let Some(head) = all_of(&file.head).await else { return };
        let Some(tail) = file.tail.lock().unwrap().clone() else { return };   // (the size came with the start)
        let Some(end) = all_of(&tail).await else { return };
        let Some((from, to)) = mkv::span(&head, &end, start) else { return };
        let len = (to - from + MID).min(3 * MID);
        let part = Part::new(from, len);
        *file.mid.lock().unwrap() = Some((start, part.clone()));
        let range = format!("bytes={from}-{}", from + len - 1);
        match client().get(&file.url).header("User-Agent", &file.agent).header("Range", range).send().await {
            Ok(r) if r.headers().get("content-range").and_then(|v| v.to_str().ok()).is_some_and(|c| c.starts_with(&format!("bytes {from}-"))) => {
                fill(part, r).await
            }
            _ => part.closed(),
        }
    });
}

/// Just enough Matroska (EBML) to find where a time's cluster starts.
mod mkv {
    const SEGMENT: u64 = 0x1853_8067;
    const TIMECODE_SCALE: u64 = 0x2A_D7B1;
    const CUES: u64 = 0x1C53_BB6B;
    const CUE_POINT: u64 = 0xBB;
    const CUE_TIME: u64 = 0xB3;
    const CUE_TRACK_POSITIONS: u64 = 0xB7;
    const CUE_CLUSTER_POSITION: u64 = 0xF1;

    /// An element's (id, data start, data size) at `at`.
    fn element(b: &[u8], at: usize) -> Option<(u64, usize, u64)> {
        let (id, n) = vint(b, at, false)?;
        let (size, m) = vint(b, at + n, true)?;
        Some((id, at + n + m, size))
    }

    /// A variable-length number: (value, its length); `strip` drops its length marker (a size, not an id).
    fn vint(b: &[u8], at: usize, strip: bool) -> Option<(u64, usize)> {
        let first = *b.get(at)?;
        let len = first.leading_zeros() as usize + 1;
        if len > 8 || at + len > b.len() {
            return None;
        }
        let mut v = if strip { (first as u64) & (0xFF >> len) } else { first as u64 };
        for &x in &b[at + 1..at + len] {
            v = (v << 8) | x as u64;
        }
        Some((v, len))
    }

    fn uint(b: &[u8]) -> u64 {
        b.iter().take(8).fold(0, |v, &x| (v << 8) | x as u64)
    }

    /// The elements in b[from..to]: (id, data).
    fn children(b: &[u8], from: usize, to: usize) -> Vec<(u64, &[u8])> {
        let mut out = Vec::new();
        let mut at = from;
        while at < to {
            let Some((id, data, size)) = element(b, at) else { break };
            let Some(end) = data.checked_add(size as usize).filter(|&e| e <= to) else { break };
            out.push((id, &b[data..end]));
            at = end;
        }
        out
    }

    /// Where an element with `id` starts (its id's bytes, first found).
    fn find(b: &[u8], id: u64) -> Option<usize> {
        let bytes = id.to_be_bytes();
        let needle = &bytes[bytes.iter().position(|&x| x != 0)?..];
        b.windows(needle.len()).position(|w| w == needle)
    }

    /// The bytes (from, to) of the cluster before `secs`' and of its own, from the file's start (`head`) and its
    /// Cues (somewhere in `tail`).
    pub fn span(head: &[u8], tail: &[u8], secs: f64) -> Option<(u64, u64)> {
        let (_, segment_data, _) = element(head, find(head, SEGMENT)?)?;
        let scale = find(head, TIMECODE_SCALE)
            .and_then(|at| element(head, at))
            .and_then(|(_, d, n)| head.get(d..d + n as usize))
            .map(uint)
            .filter(|&s| s > 0)
            .unwrap_or(1_000_000);
        let (_, data, size) = element(tail, find(tail, CUES)?)?;
        let end = data.checked_add(size as usize).filter(|&e| e <= tail.len())?;
        let mut points: Vec<(f64, u64)> = Vec::new();
        for (id, point) in children(tail, data, end) {
            if id != CUE_POINT {
                continue;
            }
            let fields = children(point, 0, point.len());
            let time = fields.iter().find(|(i, _)| *i == CUE_TIME).map(|(_, d)| uint(d));
            let pos = fields
                .iter()
                .find(|(i, _)| *i == CUE_TRACK_POSITIONS)
                .and_then(|(_, d)| children(d, 0, d.len()).into_iter().find(|(i, _)| *i == CUE_CLUSTER_POSITION))
                .map(|(_, d)| uint(d));
            if let (Some(t), Some(p)) = (time, pos) {
                points.push((t as f64 * scale as f64 / 1e9, p));
            }
        }
        // (mpv itself doesn't seek by a handful of cues)
        if points.len() < 4 {
            return None;
        }
        let i = points.iter().rposition(|(t, _)| *t <= secs)?;
        let base = segment_data as u64;
        Some((base + points[i.saturating_sub(1)].1, base + points[i].1))
    }

    #[cfg(test)]
    pub fn sample() -> (Vec<u8>, Vec<u8>) {
        // (an EBML header, then a Segment of unknown size with Info { TimecodeScale: 1 ms })
        let mut head = vec![0x1A, 0x45, 0xDF, 0xA3, 0x80];
        head.extend([0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        head.extend([0x15, 0x49, 0xA9, 0x66, 0x87, 0x2A, 0xD7, 0xB1, 0x83, 0x0F, 0x42, 0x40]);
        // (Cues at 0, 10, 20 and 30 s: clusters at 100, 2000, 4000 and 6000)
        let mut cues = Vec::new();
        for (t, p) in [(0u16, 100u16), (10_000, 2000), (20_000, 4000), (30_000, 6000)] {
            let pos = [0xF7, 0x81, 0x01, 0xF1, 0x82, (p >> 8) as u8, p as u8];
            let mut point = vec![0xB3, 0x82, (t >> 8) as u8, t as u8, 0xB7, 0x80 | pos.len() as u8];
            point.extend(pos);
            cues.push(0xBB);
            cues.push(0x80 | point.len() as u8);
            cues.extend(point);
        }
        let mut tail = vec![0xAA, 0xBB, 0xCC];
        tail.extend([0x1C, 0x53, 0xBB, 0x6B, 0x40 | (cues.len() >> 8) as u8, cues.len() as u8]);
        tail.extend(cues);
        (head, tail)
    }
}

/// One request from mpv: its range from what's kept where it can be, the rest straight from the source.
async fn serve(mut socket: TcpStream) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = socket.read(&mut chunk).await?;
        if n == 0 || buf.len() > 64 * 1024 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let mut lines = text.split("\r\n");
    let first: Vec<&str> = lines.next().unwrap_or_default().split(' ').collect();
    let (method, path) = (first.first().copied().unwrap_or(""), first.get(1).copied().unwrap_or(""));
    let range = lines
        .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("range")).map(|(_, v)| v.trim().to_string()));
    let file = path.strip_prefix("/f/").and_then(|id| relay()?.files.lock().unwrap().iter().find(|(i, _)| i == id).map(|(_, f)| f.clone()));
    let Some(file) = file else {
        return socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
    };
    let size = loop {
        let waiting = file.sized.notified();
        if let Some(size) = *file.size.lock().unwrap() {
            break Some(size);
        }
        if file.head.finished() {
            break None;
        }
        waiting.await;
    };
    let Some(size) = size else {
        return socket.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
    };
    let (from, mut to) = parse_range(range.as_deref(), size);
    // (a read from the kept start ends with it: mpv asks again for what follows; one long answer from there to the
    // end stalled mpv when it then jumped elsewhere, a resume's seek)
    if from < HEAD && to >= HEAD {
        to = HEAD - 1;
    }
    if from >= size {
        let head = format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return socket.write_all(head.as_bytes()).await;
    }
    let status = if range.is_some() || to + 1 < size { "206 Partial Content" } else { "200 OK" };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nContent-Range: bytes {from}-{to}/{size}\r\nConnection: close\r\n\r\n",
        to - from + 1
    );
    socket.write_all(head.as_bytes()).await?;
    if method == "HEAD" {
        return Ok(());
    }
    let mut at = from;
    // (from the kept start, a resume's stretch, or the kept end, while it covers where mpv reads)
    let tail = file.tail.lock().unwrap().clone();
    let mid = file.mid.lock().unwrap().as_ref().map(|(_, p)| p.clone());
    for part in [Some(file.head.clone()), mid, tail].into_iter().flatten() {
        if !part.covers(at) || at > to {
            continue;
        }
        loop {
            let waiting = part.more.notified();
            let (have, done) = (part.end(), part.finished());
            if at < have {
                let slice = {
                    let bytes = part.bytes.lock().unwrap();
                    let start = (at - part.at) as usize;
                    let end = ((to + 1).min(have) - part.at) as usize;
                    bytes[start..end].to_vec()
                };
                socket.write_all(&slice).await?;
                at += slice.len() as u64;
                if at > to {
                    return Ok(());
                }
                continue;
            }
            if done {
                break;
            }
            waiting.await;
        }
    }
    // (the rest from the source)
    if at <= to {
        let response = client()
            .get(&file.url)
            .header("User-Agent", &file.agent)
            .header("Range", format!("bytes={at}-{to}"))
            .send()
            .await
            .map_err(std::io::Error::other)?;
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(std::io::Error::other)?;
            socket.write_all(&chunk).await?;
        }
    }
    Ok(())
}

/// A Range header's first span, within `size` (none: the whole file).
fn parse_range(range: Option<&str>, size: u64) -> (u64, u64) {
    let last = size.saturating_sub(1);
    let Some(spec) = range.and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.split(',').next()) else { return (0, last) };
    let (a, b) = spec.split_once('-').unwrap_or((spec, ""));
    match (a.trim().parse::<u64>().ok(), b.trim().parse::<u64>().ok()) {
        (Some(a), Some(b)) => (a, b.min(last)),
        (Some(a), None) => (a, last),
        (None, Some(n)) => (size.saturating_sub(n), last),
        (None, None) => (0, last),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// For timing by hand: ZF_RELAY_URL served through the relay for a minute, its local address in ZF_RELAY_OUT.
    #[test]
    #[ignore]
    fn serve_for_timing() {
        let url = std::env::var("ZF_RELAY_URL").unwrap();
        let start = std::env::var("ZF_RELAY_START").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let local = local(&url, "Mozilla/5.0", start).unwrap();
        std::fs::write(std::env::var("ZF_RELAY_OUT").unwrap(), &local).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(90));
    }

    #[test]
    fn a_resume_lands_on_its_cluster_from_the_one_before() {
        let (head, tail) = mkv::sample();
        let base = 5 + 12;   // (the Segment's data: after the EBML header and the Segment's own id and size)
        assert_eq!(mkv::span(&head, &tail, 25.0), Some((base + 2000, base + 4000)));
        assert_eq!(mkv::span(&head, &tail, 35.0), Some((base + 4000, base + 6000)));
        assert_eq!(mkv::span(&head, &[0u8; 64], 25.0), None);
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range(Some("bytes=0-"), 100), (0, 99));
        assert_eq!(parse_range(Some("bytes=10-19"), 100), (10, 19));
        assert_eq!(parse_range(Some("bytes=90-200"), 100), (90, 99));
        assert_eq!(parse_range(Some("bytes=-30"), 100), (70, 99));
        assert_eq!(parse_range(None, 100), (0, 99));
    }
}
