//! Pictures: from the disk if we have them, else the network; decoded straight to the size they're shown at, off the window's
//! thread. Only the finished pixels reach the GPU and nothing large stays in RAM.
use crate::kit::Img;
use crate::net;
use crate::App;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio::sync::{OnceCell, Semaphore};
use crate::platform::{Decoder, Error, Picture, Resample, Result};

const DISK_MOST: u64 = zcore::cache::IMAGE_DISK_CAP;

fn folder() -> PathBuf {
    let p = net::cache_root().join("images");
    let _ = std::fs::create_dir_all(&p);
    p
}
fn file_for(url: &str) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    folder().join(format!("{:016x}", h.finish()))
}

fn gate() -> &'static (Arc<Semaphore>, Arc<Semaphore>) {
    static G: OnceLock<(Arc<Semaphore>, Arc<Semaphore>)> = OnceLock::new();
    G.get_or_init(|| {
        (
            Arc::new(Semaphore::new(zcore::api::picture_rules().fetches)),
            Arc::new(Semaphore::new(zcore::api::picture_decodes(
                std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2),
            ))),
        )
    })
}

type Bytes = Arc<[u8]>;
type Request = OnceCell<Option<Bytes>>;

pub struct AnimationFrame {
    pub pixels: Vec<u8>,
    pub delay: Duration,
}

pub struct DecodedImage {
    pub pixels: Vec<u8>,
    pub frames: Option<Vec<AnimationFrame>>,
}

async fn bytes(url: &str) -> Option<Bytes> {
    fetch(url, Duration::from_millis(zcore::api::picture_rules().wait_ms.into())).await
}

/// A manhwa page's size in pixels, its file fetched and kept (for drawing it) if it isn't here yet (a downloaded one,
/// "file:", read from its file). A page runs to a few MB, so it's given longer than a card's picture.
pub async fn page_size(url: &str) -> Option<(u32, u32)> {
    let data = match url.strip_prefix("file:") {
        Some(path) => Bytes::from(tokio::fs::read(path).await.ok()?),
        None => fetch(url, Duration::from_millis(zcore::api::picture_rules().page_wait_ms.into())).await?,
    };
    tokio::task::spawn_blocking(move || dimensions(&data)).await.ok().flatten()
}

async fn fetch(url: &str, wait: Duration) -> Option<Bytes> {
    static IN_FLIGHT: OnceLock<Mutex<HashMap<String, Weak<Request>>>> = OnceLock::new();
    let request = {
        let mut requests = IN_FLIGHT.get_or_init(Default::default).lock().unwrap();
        if let Some(existing) = requests.get(url).and_then(Weak::upgrade) {
            existing
        } else {
            if requests.len() >= 128 {
                requests.retain(|_, request| request.strong_count() > 0);
            }
            let request = Arc::new(OnceCell::new());
            requests.insert(url.to_owned(), Arc::downgrade(&request));
            request
        }
    };
    request
        .get_or_init(|| async {
            let file = file_for(url);
            if let Ok(bytes) = tokio::fs::read(&file).await {
                return Some(Bytes::from(bytes));
            }
            let _permit = gate().0.acquire().await.ok()?;
            let mut request = net::plain().get(url).header("User-Agent", crate::youtube::AGENT).timeout(wait);
            if zcore::manhwa::is_page(url) {
                request = request.header("Referer", zcore::manhwa::REFERER);
            }
            let response = request
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?;
            let bytes = response.bytes().await.ok()?;
            static TEMP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let id = TEMP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let temporary = file.with_extension(format!("{}.{}.part", std::process::id(), id));
            if tokio::fs::write(&temporary, &bytes).await.is_ok() {
                let _ = tokio::fs::rename(&temporary, &file).await;
            }
            Some(Bytes::from(bytes.as_ref()))
        })
        .await
        .clone()
}

// ---------- what the pictures say ----------
// What the server reads on a picture (/api/arttext), a picture's colour (/api/artcolor) and a logo's tone are kept
// for good. The core decides what to ask, how many at once, when to ask again and what the file holds
// (`zcore::art_answers`); this only does the asking and writes the file.
type Work = std::pin::Pin<Box<dyn std::future::Future<Output = Option<i64>> + Send>>;

/// One store of answers, as the core keeps it, and the work for the answers waiting for their turn.
pub struct Kept {
    kind: zcore::art_answers::Kind,
    inner: Mutex<Option<(zcore::art_answers::Answers, HashMap<String, Work>)>>,
}

pub static TEXT: Kept = Kept::new(zcore::art_answers::Kind::Text);
pub static COLOURS: Kept = Kept::new(zcore::art_answers::Kind::Colours);

impl Kept {
    const fn new(kind: zcore::art_answers::Kind) -> Self {
        Self { kind, inner: Mutex::new(None) }
    }

    fn file(&self) -> PathBuf {
        net::root().join(self.kind.file())
    }

    /// The answer for `key` if it's known; else None, and `work` is run when the core says (the page is drawn again
    /// when it comes). `work` gives None when it couldn't be answered.
    pub fn answer(&'static self, key: String, work: impl std::future::Future<Output = Option<i64>> + Send + 'static) -> Option<i64> {
        let mut inner = self.inner.lock().unwrap();
        let (answers, waiting) = inner.get_or_insert_with(|| {
            let mut answers = zcore::art_answers::Answers::new(self.kind);
            if let Ok(saved) = std::fs::read_to_string(self.file()) {
                answers.restore(&saved);
            }
            (answers, HashMap::new())
        });
        if let Some(v) = answers.get(&key) {
            return Some(v);
        }
        let start = answers.want(&key);
        let work: Work = Box::pin(work);
        if start.contains(&key) {
            drop(inner);
            self.run(key, work);
        } else if answers.is_waiting(&key) {
            waiting.insert(key, work);
        }
        None
    }

    fn run(&'static self, key: String, work: Work) {
        crate::spawn(async move {
            let answer = work.await;
            let (done, next) = {
                let mut inner = self.inner.lock().unwrap();
                let Some((answers, waiting)) = inner.as_mut() else { return };
                let done = answers.done(&key, answer);
                let next: Vec<(String, Work)> = done.start.iter().filter_map(|k| waiting.remove(k).map(|w| (k.clone(), w))).collect();
                (done, next)
            };
            if let Some(saved) = done.save {
                net::write_soon(self.file(), saved);
            }
            if done.changed {
                crate::ui(|a| a.kit.dirty = true);
            }
            for (k, w) in next {
                self.run(k, w);
            }
            if let Some(ms) = done.retry_ms {
                tokio::time::sleep(Duration::from_millis(ms.into())).await;
                if let Some((answers, _)) = self.inner.lock().unwrap().as_mut() {
                    answers.retry(&key);
                }
            }
        });
    }
}

/// What writing a picture of `title` has on it, as the server read it (`zcore::art::writing`). None: not read yet
/// (being read; the page is drawn again then).
pub fn art_text(url: &str, title: &str) -> Option<zcore::art::Writing> {
    if url.is_empty() {
        return Some(zcore::art::Writing::Clean);
    }
    let (title, address) = (title.to_string(), url.to_string());
    let code = TEXT.answer(zcore::art_answers::text_key(url), async move {
        let answer = net::raw(&zcore::art::path(&address)).await.ok()?;
        zcore::art::writing_code(&answer, &title)
    })?;
    Some(zcore::art::Writing::from_code(code))
}

/// Whether a logo is in a script the viewers read (`zcore::art::latin`). None: not known yet.
pub fn latin_logo(url: &str) -> Option<bool> {
    if url.is_empty() {
        return Some(false);
    }
    let address = url.to_string();
    TEXT.answer(zcore::art_answers::logo_key(url), async move {
        match net::raw(&zcore::art::path(&address)).await {
            Ok(answer) => zcore::art::latin_code(Some(&answer), false),
            Err(e) => zcore::art::latin_code(None, e.status == 400),
        }
    })
    .map(|v| v == 1)
}

/// How a logo is shown over the cards' dark shade, so it always reads (None until known): as it is; a dark one
/// (Heart of the Beast's deep teal) brightened, its colour kept; a black one lifted to white. (brightness, lift)
pub fn logo_tone(url: &str) -> Option<(f32, f32)> {
    if url.is_empty() {
        return Some((1., 0.));
    }
    let address = url.to_string();
    let code = TEXT.answer(zcore::art_answers::tone_key(url), async move {
        let data = bytes(&address).await?;
        tokio::task::spawn_blocking(move || {
            let side = zcore::pictures::TONE_SIDE;
            let pixels = decode(&data, side, side / 2, Decoding { contain: true, ..Default::default() }).ok()?;
            Some(zcore::pictures::tone_code(zcore::pictures::peak(&straight(&pixels))))
        })
        .await
        .ok()
        .flatten()
    })?;
    Some(match zcore::pictures::tone_of_code(code) {
        zcore::pictures::Tone::AsIs => (1., 0.),
        zcore::pictures::Tone::Brighten { by } => (by, 0.),
        zcore::pictures::Tone::Lift { by } => (1., by),
    })
}

/// Pictures read into the cache all at once, `done` once every one has come (or failed).
pub fn warm_all(urls: Vec<String>) {
    crate::spawn(async move {
        futures_util::future::join_all(urls.iter().map(|u| bytes(u))).await;
    });
}

/// A picture read ahead into the cache `after_ms` from now.
pub fn preload(url: String, after_ms: u32) {
    crate::spawn(async move {
        tokio::time::sleep(Duration::from_millis(after_ms.into())).await;
        let _ = bytes(&url).await;
    });
}

/// A WebP's pixels as a picture to carry on with (the OS's decoder may not read WebP).
fn webp_source(data: &[u8]) -> Option<Picture> {
    let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(data)).ok()?;
    let (width, height) = decoder.dimensions();
    let size = decoder
        .output_buffer_size()
        .filter(|size| *size <= 96 * 1024 * 1024)?;
    let mut pixels = vec![0; size];
    decoder.read_image(&mut pixels).ok()?;
    Picture::from_pixels(width, height, &pixels, decoder.has_alpha())
}

fn avatar_gif(data: &[u8], w: u32, h: u32) -> Option<Vec<AnimationFrame>> {
    use gif::{ColorOutput, DecodeOptions, DisposalMethod, MemoryLimit};
    /// The last frame's box (left, top, width, height), how it's disposed of, and the canvas from before it.
    type GifPrevious = ((usize, usize, usize, usize), DisposalMethod, Option<Vec<u8>>);
    if w == 0 || h == 0 || w as u64 * h as u64 > 4_000_000 {
        return None;
    }
    let mut options = DecodeOptions::new();
    options.set_color_output(ColorOutput::RGBA);
    options.check_frame_consistency(true);
    options.set_memory_limit(MemoryLimit::Bytes(std::num::NonZeroU64::new(32_000_000)?));
    let mut decoder = options.read_info(std::io::Cursor::new(data)).ok()?;
    let (sw, sh) = (decoder.width() as usize, decoder.height() as usize);
    if sw == 0 || sh == 0 || sw.checked_mul(sh)?.checked_mul(4)? > 16 * 1024 * 1024 {
        return None;
    }
    let mut canvas = vec![0u8; sw * sh * 4];
    let mut previous: Option<GifPrevious> = None;
    let mut frames = Vec::new();
    let mut sample_every = 1usize;
    let mut frame_number = 0usize;
    while let Some(frame) = decoder.read_next_frame().ok()? {
        if frame_number >= 10_000 {
            break;
        }
        if let Some(((left, top, fw, fh), disposal, saved)) = previous.take() {
            match disposal {
                DisposalMethod::Background => {
                    for y in top..top + fh {
                        canvas[(y * sw + left) * 4..(y * sw + left + fw) * 4].fill(0);
                    }
                }
                DisposalMethod::Previous => {
                    if let Some(saved) = saved { canvas = saved; }
                }
                DisposalMethod::Any | DisposalMethod::Keep => {}
            }
        }
        let (left, top, fw, fh) = (frame.left as usize, frame.top as usize, frame.width as usize, frame.height as usize);
        if left + fw > sw || top + fh > sh || frame.buffer.len() != fw * fh * 4 {
            return None;
        }
        let saved = (frame.dispose == DisposalMethod::Previous).then(|| canvas.clone());
        for y in 0..fh {
            for x in 0..fw {
                let src = (y * fw + x) * 4;
                let dst = ((top + y) * sw + left + x) * 4;
                let a = frame.buffer[src + 3] as u32;
                if a == 255 {
                    canvas[dst..dst + 4].copy_from_slice(&frame.buffer[src..src + 4]);
                } else if a != 0 {
                    let back = 255 - a;
                    for c in 0..3 {
                        canvas[dst + c] = ((frame.buffer[src + c] as u32 * a + canvas[dst + c] as u32 * back) / 255) as u8;
                    }
                    canvas[dst + 3] = (a + canvas[dst + 3] as u32 * back / 255).min(255) as u8;
                }
            }
        }
        let pixels = resize_avatar(&canvas, sw, sh, w as usize, h as usize)?;
        let delay = Duration::from_millis((frame.delay as u64 * 10).clamp(20, 10_000));
        if frame_number.is_multiple_of(sample_every) {
            frames.push(AnimationFrame { pixels, delay });
        } else if let Some(last) = frames.last_mut() {
            last.delay += delay;
        }
        previous = Some(((left, top, fw, fh), frame.dispose, saved));
        frame_number += 1;
        if frames.len() > 180 {
            let mut compact = Vec::with_capacity(frames.len().div_ceil(2));
            let mut iter = frames.into_iter();
            while let Some(first) = iter.next() {
                if let Some(mut second) = iter.next() {
                    second.delay += first.delay;
                    compact.push(second);
                } else {
                    compact.push(first);
                }
            }
            frames = compact;
            sample_every = sample_every.saturating_mul(2);
        }
    }
    (!frames.is_empty()).then_some(frames)
}

/// An animated WebP's frames (a profile picture: the server makes uploaded GIFs into these), each the whole picture
/// as it stands after that frame, sized to the box; None for a still one.
fn avatar_webp(data: &[u8], w: u32, h: u32) -> Option<Vec<AnimationFrame>> {
    if w == 0 || h == 0 || w as u64 * h as u64 > 4_000_000 {
        return None;
    }
    let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(data)).ok()?;
    if !decoder.is_animated() {
        return None;
    }
    let (sw, sh) = decoder.dimensions();
    let (sw, sh) = (sw as usize, sh as usize);
    let size = decoder.output_buffer_size()?;
    if sw == 0 || sh == 0 || size > 16 * 1024 * 1024 {
        return None;
    }
    let mut buffer = vec![0u8; size];
    let mut frames = Vec::new();
    for _ in 0..decoder.num_frames().min(300) {
        let delay = decoder.read_frame(&mut buffer).ok()?;
        // (RGBA as it comes; a picture without transparency comes as RGB, given its alpha here)
        let rgba: Vec<u8> = if size == sw * sh * 4 {
            buffer.clone()
        } else {
            buffer.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect()
        };
        frames.push(AnimationFrame { pixels: resize_avatar(&rgba, sw, sh, w as usize, h as usize)?, delay: Duration::from_millis((delay as u64).clamp(20, 10_000)) });
    }
    (!frames.is_empty()).then_some(frames)
}

fn resize_avatar(src: &[u8], sw: usize, sh: usize, w: usize, h: usize) -> Option<Vec<u8>> {
    if sw == 0 || sh == 0 || w == 0 || h == 0 { return None; }
    let target_ratio = w as f64 / h as f64;
    let source_ratio = sw as f64 / sh as f64;
    let (cw, ch) = if source_ratio > target_ratio {
        ((sh as f64 * target_ratio).max(1.), sh as f64)
    } else {
        (sw as f64, (sw as f64 / target_ratio).max(1.))
    };
    let left = (sw as f64 - cw) * 0.5;
    let top = (sh as f64 - ch) * 0.5;
    let mut out = vec![0; w.checked_mul(h)?.checked_mul(4)?];
    for y in 0..h {
        let sy = (top + (y as f64 + 0.5) * ch / h as f64 - 0.5).clamp(0., (sh - 1) as f64);
        let y0 = sy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let fy = sy - y0 as f64;
        for x in 0..w {
            let sx = (left + (x as f64 + 0.5) * cw / w as f64 - 0.5).clamp(0., (sw - 1) as f64);
            let x0 = sx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let fx = sx - x0 as f64;
            let weights = [(x0, y0, (1. - fx) * (1. - fy)), (x1, y0, fx * (1. - fy)), (x0, y1, (1. - fx) * fy), (x1, y1, fx * fy)];
            let mut rgba = [0f64; 4];
            for (px, py, weight) in weights {
                let at = (py * sw + px) * 4;
                let alpha = src[at + 3] as f64 / 255.;
                rgba[0] += src[at] as f64 * alpha * weight;
                rgba[1] += src[at + 1] as f64 * alpha * weight;
                rgba[2] += src[at + 2] as f64 * alpha * weight;
                rgba[3] += src[at + 3] as f64 * weight;
            }
            let at = (y * w + x) * 4;
            out[at] = rgba[2].round().clamp(0., 255.) as u8;
            out[at + 1] = rgba[1].round().clamp(0., 255.) as u8;
            out[at + 2] = rgba[0].round().clamp(0., 255.) as u8;
            out[at + 3] = rgba[3].round().clamp(0., 255.) as u8;
        }
    }
    Some(out)
}

/// The part of a picture that isn't see-through (None if all of it is, or none of it is).
fn opaque_bounds(input: &Picture) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = input.size().ok()?;
    if w == 0 || h == 0 || w as u64 * h as u64 > 8_000_000 {
        return None;
    }
    let mut px = vec![0u8; (w * h * 4) as usize];
    input.read(w * 4, &mut px).ok()?;
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for y in 0..h {
        for x in 0..w {
            if px[((y * w + x) * 4 + 3) as usize] > 8 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x1 < x0 || (x0 == 0 && y0 == 0 && x1 == w - 1 && y1 == h - 1) {
        return None;
    }
    Some((x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// What's baked into a picture as it's decoded: its tone and how it's fitted to its box (`kit::Img`'s fields).
#[derive(Clone, Copy)]
pub struct Decoding {
    pub gray: f32,
    pub bright: f32,
    pub lift: f32,
    pub crop: Option<(u32, u32, u32, u32)>,
    pub contain: bool,
    pub crop_y: f32,
    pub centred: bool,
}
impl Default for Decoding {
    fn default() -> Self {
        Decoding { gray: 0., bright: 1., lift: 0., crop: None, contain: false, crop_y: 0.5, centred: false }
    }
}

/// Crops to fill, as ContentScale.Crop does, then shrinks. BGRA, premultiplied.
pub fn decode(data: &[u8], w: u32, h: u32, how: Decoding) -> Result<Vec<u8>> {
    let Decoding { gray, bright, lift, crop, contain, crop_y, centred } = how;
    let mut input = if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        webp_source(data).ok_or_else(|| Error::new("unreadable WebP"))?
    } else {
        Picture::decode(data)?
    };
    // (a big JPEG read at a half, a quarter or an eighth while it's decoded — far quicker than reading it whole
    // and shrinking it after — but only down to a size still at least as big as it's shown, so the scaler
    // below still makes the last step and nothing comes out softer)
    if crop.is_none() && !contain {
        if let Some(smaller) = input.smaller(w, h) {
            input = smaller;
        }
    }
    if let Some((x, y, width, height)) = crop {
        input = input.crop(x, y, width, height)?;
    } else if contain {
        // (a logo's file often has wide empty margins: fitted with them, the logo comes out tiny)
        if let Some((x, y, width, height)) = opaque_bounds(&input) {
            input = input.crop(x, y, width, height)?;
        }
    }
    let (mut sw, mut sh) = input.size()?;
    if sw == 0 || sh == 0 {
        return Err(Error::new("empty picture"));
    }
    // (a crop is shown whole at the size asked, stretched by the hair its box differs: trimmed to fill instead,
    // a manhwa page's pieces each lost a row or two at their edges, and the art jumped where they met)
    let (tw, th) = if crop.is_some() {
        (w, h)
    } else if contain {
        let scale = (w as f64 / sw as f64).min(h as f64 / sh as f64);
        (
            ((sw as f64 * scale).round() as u32).clamp(1, w),
            ((sh as f64 * scale).round() as u32).clamp(1, h),
        )
    } else {
        let cw = ((sh as u64 * w as u64 / h as u64).min(sw as u64) as u32).max(1);
        let ch = ((sw as u64 * h as u64 / w as u64).min(sh as u64) as u32).max(1);
        if (cw, ch) != (sw, sh) {
            let top = ((sh - ch) as f32 * crop_y).round() as u32;
            input = input.crop((sw - cw) / 2, top, cw, ch)?;
            (sw, sh) = (cw, ch);
        }
        (w, h)
    };
    if (tw, th) != (sw, sh) {
        let how = if w.max(h) <= 480 && crop.is_none() { Resample::Fant } else { Resample::Cubic };
        input = input.resize(tw, th, how)?;
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
    if contain {
        let mut fitted = vec![0u8; (tw * th * 4) as usize];
        input.read(tw * 4, &mut fitted)?;
        // (fitted where `crop_y` says: .5 in the middle, 0 at the left and the foot: a logo standing on its line)
        let row = (tw * 4) as usize;
        // (or `centred`: in the middle across, on its foot)
        let left = (((w - tw) as f32 * if centred { 0.5 } else { crop_y }).round() as usize) * 4;
        let top = ((h - th) as f32 * if centred { 1. } else { 1. - crop_y }).round() as usize;
        for (y, src) in fitted.chunks_exact(row).enumerate() {
            let at = (y + top) * (w * 4) as usize + left;
            out[at..at + row].copy_from_slice(src);
        }
    } else {
        input.read(w * 4, &mut out)?;
    }
    if gray != 0. || bright != 1. || lift != 0. {
        for p in out.as_chunks_mut::<4>().0 {
            let (b, g, r, a) = (p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32);
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            // (premultiplied: never brighter than its own alpha; lifted toward white as far as its alpha goes)
            let tone = |c: f32| {
                let c = ((c + (l - c) * gray) * bright).min(a);
                (c + (a - c) * lift).min(a)
            };
            p[0] = tone(b) as u8;
            p[1] = tone(g) as u8;
            p[2] = tone(r) as u8;
        }
    }
    Ok(out)
}

pub fn load(
    img: Img,
    w: u32,
    h: u32,
    done: impl FnOnce(&mut App, Option<DecodedImage>) + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    crate::spawn(async move {
        let primary = picture(&img.url, &img, w, h).await;
        let pixels = if primary.is_none() {
            match img.fallback.as_ref().filter(|url| *url != &img.url) {
                Some(url) => picture(url, &img, w, h).await,
                None => None,
            }
        } else {
            primary
        };
        crate::ui(move |app| done(app, pixels));
    })
}

/// The stage's shading, baked into its picture: darker toward the left (where its words are) and toward its foot.
/// (Colour channels only; the pixels' alpha, and so the rounded corners, stay as they are.)
fn shade_stage(pixels: &mut [u8], w: usize, h: usize, kind: u8) {
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return;
    }
    let ramp = |at: f32, stops: &[(f32, f32)]| -> f32 {
        if at <= stops[0].0 { return stops[0].1; }
        for pair in stops.windows(2) {
            let ((a, va), (b, vb)) = (pair[0], pair[1]);
            if at <= b {
                return va + (vb - va) * ((at - a) / (b - a).max(1e-6));
            }
        }
        stops[stops.len() - 1].1
    };
    // (1: the stage, darker toward its left and foot; 2: a poster, deep toward its foot for its name; 3: a landscape
    // card, darker toward its foot for its logo)
    let across: Vec<f32> = (0..w).map(|x| if kind == 1 { 1. - ramp(x as f32 / w as f32, &[(0., 0.78), (0.26, 0.52), (0.5, 0.08), (1., 0.)]) } else { 1. }).collect();
    // (4: a title's band: the picture itself fading to nothing toward its left and its foot, into the page's own
    // ground: its alpha, baked in, with a little dither so the fall never bands)
    if kind == 4 {
        let left: &[(f32, f32)] = &[(0., 0.), (0.08, 0.06), (0.2, 0.25), (0.35, 0.55), (0.5, 0.85), (0.62, 1.)];
        let foot: &[(f32, f32)] = &[(0., 1.), (0.45, 1.), (0.6, 0.8), (0.72, 0.55), (0.85, 0.24), (0.95, 0.06), (1., 0.)];
        let across: Vec<f32> = (0..w).map(|x| ramp(x as f32 / w as f32, left)).collect();
        let mut seed = 0x9E37_79B9u32;
        for y in 0..h {
            let down = ramp(y as f32 / h as f32, foot);
            let row = &mut pixels[y * w * 4..(y + 1) * w * 4];
            for (x, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5;
                let noise = (seed & 0xFF) as f32 / 255. - 0.5;
                let k = (across[x] * down).clamp(0., 1.);
                for c in p.iter_mut() {
                    *c = (*c as f32 * k + noise).round().clamp(0., 255.) as u8;
                }
            }
        }
        return;
    }
    let foot: &[(f32, f32)] = match kind { 2 => &[(0.55, 0.), (0.85, 0.55), (1., 0.7)], 3 => &[(0.35, 0.), (1., 0.8)], _ => &[(0.55, 0.), (1., 0.55)] };
    for y in 0..h {
        let down = 1. - ramp(y as f32 / h as f32, foot);
        let row = &mut pixels[y * w * 4..(y + 1) * w * 4];
        for (x, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let k = across[x] * down;
            for c in &mut p[..3] {
                *c = (*c as f32 * k).round() as u8;
            }
        }
    }
}

/// A logo's own colour, as a hue (0-254), worked out once and remembered: its vivid, lit letters' hue, weighted by how
/// vivid and lit they are; 255 when it's white, grey or black (nothing to take a colour from). None until known.
pub fn logo_hue(url: &str) -> Option<u8> {
    if url.is_empty() {
        return Some(255);
    }
    let address = url.to_string();
    let code = TEXT.answer(zcore::art_answers::hue_key(url), async move {
        let data = bytes(&address).await?;
        tokio::task::spawn_blocking(move || {
            let side = zcore::pictures::HUE_SIDE;
            let pixels = decode(&data, side, side / 2, Decoding { contain: true, ..Default::default() }).ok()?;
            // (kept as 0-254 of the circle, 255 for none: the answers on disk stay readable)
            Some(zcore::pictures::logo_hue(&straight(&pixels)).map_or(255, |deg| ((deg / 360.) * 254.).round().min(254.) as i64))
        })
        .await
        .ok()
        .flatten()
    })?;
    Some(code.clamp(0, 255) as u8)
}

/// A chapter's own still, for one the server has none for (`zcore::manhwa::still`): "file:…" once it's made and kept;
/// None while it's being made (the page is drawn again then), or when it can't be.
pub fn chapter_still(path: &str) -> Option<String> {
    let file = crate::disk::path(&zcore::manhwa::still::file(path));
    if file.is_file() {
        return Some(format!("file:{}", file.display()));
    }
    static ASKED: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);
    if ASKED.lock().unwrap().get_or_insert_with(Default::default).insert(path.to_string()) {
        let path = path.to_string();
        crate::spawn(async move {
            static MAKING: OnceLock<Semaphore> = OnceLock::new();
            let Ok(_turn) = MAKING.get_or_init(|| Semaphore::new(zcore::manhwa::still::AT_ONCE)).acquire().await else { return };
            if make_still(&path, file).await.is_some() {
                crate::ui(|a| a.kit.dirty = true);
            }
        });
    }
    None
}

/// The pages the core asks for decoded at the size it gives, the window it picks cropped, and kept at `file`.
async fn make_still(path: &str, file: PathBuf) -> Option<()> {
    use zcore::manhwa::still::{self, Still};
    let saved = crate::downloads::chapter_pages(path);
    let pages: Vec<String> = match &saved {
        Some(files) => files.iter().map(|f| format!("file:{}", f.display())).collect(),
        None => zcore::manhwa::parse_pages(&net::raw(&zcore::manhwa::pages_path(path)).await.ok()?).ok()?,
    };
    let mut picking = Still::new(pages.len(), saved.is_some());
    let mut best: Option<(Vec<u8>, u32, u32)> = None;
    while let Some(i) = picking.next() {
        let data = match pages[i].strip_prefix("file:") {
            Some(f) => tokio::fs::read(f).await.ok().map(Bytes::from),
            None => bytes(&pages[i]).await,
        };
        let page = match data {
            Some(data) => tokio::task::spawn_blocking(move || {
                let (w, h) = dimensions(&data)?;
                let (w, h) = still::fit(w, h);
                Some((straight(&decode(&data, w, h, Decoding::default()).ok()?), w, h))
            })
            .await
            .ok()
            .flatten(),
            None => None,
        };
        let (w, h) = page.as_ref().map_or((0, 0), |p| (p.1, p.2));
        if let (Some(window), Some((rgba, ..))) = (picking.looked(page.as_ref().map(|p| p.0.as_slice()), w, h), &page) {
            let rows = &rgba[(window.top * w * 4) as usize..((window.top + window.height) * w * 4) as usize];
            best = Some((rows.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect(), w, window.height));
        }
    }
    let (rgb, w, h) = best?;
    let mut jpeg = Vec::new();
    jpeg_encoder::Encoder::new(&mut jpeg, still::QUALITY).encode(&rgb, u16::try_from(w).ok()?, u16::try_from(h).ok()?, jpeg_encoder::ColorType::Rgb).ok()?;
    tokio::fs::create_dir_all(file.parent()?).await.ok()?;
    // (beside it first, so a still cut short never sits under its name)
    let part = file.with_extension("jpg.part");
    tokio::fs::write(&part, &jpeg).await.ok()?;
    tokio::fs::rename(&part, &file).await.ok()
}

/// Premultiplied BGRA pixels, as decoded, as the straight RGBA the core judges pictures by.
fn straight(bgra: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bgra.len());
    for p in bgra.as_chunks::<4>().0 {
        let a = p[3];
        let un = |c: u8| if a == 0 { 0 } else { ((u16::from(c) * 255 + u16::from(a) / 2) / u16::from(a)).min(255) as u8 };
        out.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), a]);
    }
    out
}

/// The page's glow ("glow:mask"): how strongly a colour lights the window from the top left, falling away smoothly to
/// nothing, with a fine dither so it never bands. Made once at the size it's shown; every colour is this, filled
/// with its colour by the compositor (the kit's "glow:RRGGBB").
fn glow_mask(w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w.max(1) as usize, h.max(1) as usize);
    let mut out = vec![0u8; w * h * 4];
    let mut seed = 0x1234_5678u32;
    for yy in 0..h {
        for xx in 0..w {
            let dx = (xx as f32 / w as f32 - 0.22) / 0.95;
            let dy = (yy as f32 / h as f32 + 0.12) / 0.9;
            let d = (dx * dx + dy * dy).sqrt();
            let f = (1. - d).clamp(0., 1.);
            let a = 0.13 * f * f * (3. - 2. * f);
            seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5;
            let n = (seed & 0xFF) as f32 / 255. - 0.5;
            let i = (yy * w + xx) * 4;
            let alpha = (255. * a + n).round().clamp(0., 255.) as u8;
            out[i..i + 4].copy_from_slice(&[alpha; 4]);
        }
    }
    out
}

async fn picture(url: &str, img: &Img, w: u32, h: u32) -> Option<DecodedImage> {
    if url == "glow:mask" {
        return tokio::task::spawn_blocking(move || DecodedImage { pixels: glow_mask(w, h), frames: None }).await.ok();
    }
    let data = if let Some(name) = url.strip_prefix("res:") {
        Bytes::from(asset(name)?)
    } else if let Some(path) = url.strip_prefix("file:") {
        Bytes::from(tokio::fs::read(path).await.ok()?)
    } else {
        bytes(url).await?
    };
    let _permit = gate().1.acquire().await.ok()?;
    let how = Decoding { gray: img.gray, bright: img.bright, lift: img.lift, crop: img.crop, contain: img.contain, crop_y: img.crop_y, centred: img.centred };
    let animated = img.animated;
    let shade = img.shade;
    tokio::task::spawn_blocking(move || {
        let _permit = _permit;
        let gif = animated && zcore::profiles::is_gif(&data);
        let webp = animated && data.len() > 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP";
        let frames = if gif { avatar_gif(&data, w, h) } else if webp { avatar_webp(&data, w, h) } else { None };
        if let Some(mut frames) = frames {
            if frames.len() > 1 {
                return Some(DecodedImage { pixels: frames[0].pixels.clone(), frames: Some(frames) });
            }
            if let Some(frame) = frames.pop() {
                return Some(DecodedImage { pixels: frame.pixels, frames: None });
            }
        }
        decode(&data, w, h, how).ok().map(|mut pixels| {
            if shade != 0 {
                shade_stage(&mut pixels, w as usize, h as usize, shade);
            }
            DecodedImage { pixels, frames: None }
        })
    })
    .await
    .ok()
    .flatten()
}

fn asset(name: &str) -> Option<Vec<u8>> {
    match name {
        "wall" => Some(include_bytes!("../assets/images/wall.jpg").to_vec()),
        "icon" => Some(include_bytes!("../assets/images/icon.png").to_vec()),
        "pattern" => Some(include_bytes!("../assets/images/pattern.webp").to_vec()),
        // (lossless WebP: the very same pixels as the PNGs they were, 40% smaller in the exe)
        "backdrop" => Some(include_bytes!("../assets/images/backdrop.webp").to_vec()),
        "caption_preview" => Some(include_bytes!("../assets/images/caption_preview.webp").to_vec()),
        _ => None,
    }
}

/// Oldest go first, once in a while. The disk copy is what makes a second visit instant.
pub fn trim_disk() {
    std::thread::spawn(|| net::trim_folder(&folder(), DISK_MOST));
}

pub fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        return Some(
            image_webp::WebPDecoder::new(std::io::Cursor::new(data))
                .ok()?
                .dimensions(),
        );
    }
    let (w, h) = Picture::decode(data).ok()?.size().ok()?;
    (w > 0 && h > 0).then_some((w, h))
}

/// The picture at `path` as it's sent for a profile, by the core's rules (`zcore::profiles`): a GIF whole, with its
/// square; anything else that square as a small JPEG.
pub fn avatar(path: &str, square: zcore::profiles::Square) -> Option<zcore::profiles::Picture> {
    use zcore::profiles::{crop_query, Picture, AVATAR_MOST_BYTES, AVATAR_QUALITIES, AVATAR_SIDE, PICTURE_MOST_BYTES};
    let zcore::profiles::Square { x, y, side } = square;
    let data = std::fs::read(path)
        .ok()
        .filter(|d| d.len() <= PICTURE_MOST_BYTES)?;
    if zcore::profiles::is_gif(&data) {
        return Some(Picture { bytes: data, content_type: "image/gif".into(), crop: Some(crop_query(x, y, side)) });
    }
    let out = side.min(AVATAR_SIDE);
    let rgba = decode(&data, out, out, Decoding { crop: Some((x, y, side, side)), ..Default::default() }).ok()?;
    let rgb: Vec<u8> = rgba
        .as_chunks::<4>().0.iter()
        .flat_map(|p| [p[2], p[1], p[0]])
        .collect();
    for quality in AVATAR_QUALITIES {
        let mut jpeg = Vec::new();
        jpeg_encoder::Encoder::new(&mut jpeg, quality)
            .encode(&rgb, out as u16, out as u16, jpeg_encoder::ColorType::Rgb)
            .ok()?;
        if jpeg.len() <= AVATAR_MOST_BYTES {
            return Some(Picture { bytes: jpeg, content_type: "image/jpeg".into(), crop: None });
        }
    }
    None
}
