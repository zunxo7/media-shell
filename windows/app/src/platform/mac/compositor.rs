//! The visual tree as Core Animation layers, played on the window server's own clock as Windows' compositor plays
//! its visuals: each visual two layers (one placed and transformed as Windows places it, one inside scaled and
//! turned about its centre, faded, clipped, holding the brush and the children), brushes as layer contents, colours
//! and filters, and keyframes as `CAKeyframeAnimation`s with the same curves, steps, delays and repeats.
use super::draw::{self, Canvas};
use super::text::Text;
use super::window::{self, Window};
use crate::gfx::TextStyle;
use crate::platform::{self as seam, Animation, Clip, Easing, Error, Fit, Pair, Result, Scalar};
use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, AllocAnyThread, Message};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreate, CGBitmapContextCreateImage, CGColor, CGContext, CGImage, CGMutablePath};
use objc2_core_image::CIFilter;
use objc2_foundation::{NSArray, NSNumber, NSObject, NSObjectNSKeyValueCoding, NSString, NSValue};
use objc2_quartz_core::{
    kCAFillModeBackwards, kCAMediaTimingFunctionLinear, CACurrentMediaTime, CAKeyframeAnimation, CALayer, CAMediaTiming, CAMediaTimingFunction,
    CATransform3D,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::{Rc, Weak as RcWeak};

define_class!(
    /// A layer that never animates on its own: only the animations the app starts play, as on Windows.
    // SAFETY: CALayer can be subclassed; nothing here is dropped specially.
    #[unsafe(super(CALayer, NSObject))]
    #[name = "ZunflixLayer"]
    struct StillLayer;

    impl StillLayer {
        #[unsafe(method_id(actionForKey:))]
        fn action_for_key(&self, _key: &NSString) -> Option<Retained<AnyObject>> {
            None
        }
    }
);

fn new_layer() -> Retained<CALayer> {
    let layer: Retained<StillLayer> = unsafe { msg_send![StillLayer::alloc(), init] };
    layer.into_super()
}

/// The view's own layer, its y running down as the app lays out.
pub(super) fn host_layer() -> Retained<CALayer> {
    let layer = new_layer();
    layer.setGeometryFlipped(true);
    layer
}

/// Points per pixel: the seam speaks in pixels, Core Animation in points.
fn pt(px: f64) -> f64 {
    px / window::scale()
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn object<T: objc2::Message>(r: Retained<T>) -> Retained<AnyObject> {
    unsafe { Retained::cast_unchecked(r) }
}

fn number(v: f64) -> Retained<AnyObject> {
    object(NSNumber::new_f64(v))
}

fn point(x: f64, y: f64) -> Retained<AnyObject> {
    object(unsafe { NSValue::valueWithPoint(CGPoint::new(x, y)) })
}

fn rect_value(r: CGRect) -> Retained<AnyObject> {
    object(unsafe { NSValue::valueWithRect(r) })
}

fn cg_object(c: &CGColor) -> Retained<AnyObject> {
    unsafe { Retained::retain(c as *const CGColor as *mut AnyObject) }.expect("a colour")
}

fn image_object(i: &CGImage) -> Retained<AnyObject> {
    unsafe { Retained::retain(i as *const CGImage as *mut AnyObject) }.expect("an image")
}

/// A visual's layers.
#[derive(Clone)]
pub struct Visual(Rc<Node>);

struct Node {
    /// Placed at the offset, carrying the transform matrix; zero-sized, so it turns and scales from the visual's top
    /// left as Windows' matrix does.
    place: Retained<CALayer>,
    /// The visual itself, inside: sized, scaled and turned about its centre, faded, clipped; the brush's layer at
    /// its bottom, then the children.
    body: Retained<CALayer>,
    sprite: bool,
    st: RefCell<State>,
}

#[derive(Default)]
struct State {
    /// Size and centre in pixels.
    size: (f64, f64),
    centre: (f64, f64),
    /// What shows the brush (below the children), when it isn't a plain colour on the body.
    content: Option<Retained<CALayer>>,
    brush: Option<Brush>,
    inset: Option<Inset>,
}

/// An inset clip as two layers: `mask`, its left edge moved in, clipping `keep`, whose right edge is moved in.
struct Inset {
    mask: Retained<CALayer>,
    keep: Retained<CALayer>,
    /// The right inset (pixels), kept so a new size keeps it.
    right: f64,
}

/// What can be animated on a visual: the layer, its key path, and how a seam value becomes the layer's.
fn targets(v: &Node, prop: Scalar) -> Vec<(Retained<CALayer>, &'static str)> {
    let st = v.st.borrow();
    match prop {
        Scalar::OffsetX => vec![(v.place.clone(), "position.x")],
        Scalar::OffsetY => vec![(v.place.clone(), "position.y")],
        Scalar::Opacity => vec![(v.body.clone(), "opacity")],
        Scalar::Rotation => vec![(v.body.clone(), "transform.rotation.z")],
        Scalar::ClipLeft => st.inset.as_ref().map_or(vec![], |i| vec![(i.mask.clone(), "position.x"), (i.mask.clone(), "bounds.origin.x")]),
        Scalar::ClipRight => st.inset.as_ref().map_or(vec![], |i| vec![(i.keep.clone(), "bounds.size.width")]),
    }
}

impl Node {
    /// The body's pivot moved to the centre, its bounds to the size.
    fn lay_out(&self) {
        let st = self.st.borrow();
        let (w, h) = st.size;
        let (cx, cy) = st.centre;
        let anchor = CGPoint::new(if w > 0. { cx / w } else { 0.5 }, if h > 0. { cy / h } else { 0.5 });
        self.body.setAnchorPoint(anchor);
        self.body.setPosition(CGPoint::new(pt(cx), pt(cy)));
        self.body.setBounds(CGRect::new(CGPoint::new(0., 0.), CGSize::new(pt(w), pt(h))));
    }

    /// A brush's layer (and an inset clip's) fitted to a new size.
    fn fit_parts(&self) {
        let st = self.st.borrow();
        let bounds = self.body.bounds();
        if let (Some(content), Some(brush)) = (&st.content, &st.brush) {
            content.setFrame(bounds);
            brush.fit(content, st.size);
        }
        if let Some(inset) = &st.inset {
            inset.mask.setBounds(CGRect::new(CGPoint::new(inset.mask.bounds().origin.x, 0.), CGSize::new(1e7, bounds.size.height)));
            inset.keep.setFrame(CGRect::new(CGPoint::new(0., 0.), CGSize::new(pt((st.size.0 - inset.right).max(0.)), bounds.size.height)));
        }
    }
}

/// The value at `path` now: as it shows, if something's playing on it, else as it's set.
fn current(layer: &CALayer, path: &str) -> Option<Retained<AnyObject>> {
    let key = ns(path);
    let playing = unsafe { layer.animationForKey(&key) }.is_some();
    let shown = if playing { unsafe { layer.presentationLayer() } } else { None };
    shown.as_deref().unwrap_or(layer).valueForKeyPath(&key)
}

/// Set straight, any animation on it stopped first (setting a property stops its animation, as on Windows).
fn set(layer: &CALayer, path: &str, value: &AnyObject) {
    let key = ns(path);
    layer.removeAnimationForKey(&key);
    unsafe { layer.setValue_forKeyPath(Some(value), &key) };
}

/// A curve as Core Animation's timing function, made once.
fn timing(e: Easing) -> Retained<CAMediaTimingFunction> {
    thread_local! {
        static CURVES: RefCell<HashMap<[u32; 4], Retained<CAMediaTimingFunction>>> = RefCell::new(HashMap::new());
    }
    let b = match e {
        Easing::Bezier(b) => b,
        Easing::Linear | Easing::Step => return CAMediaTimingFunction::functionWithName(unsafe { kCAMediaTimingFunctionLinear }),
    };
    CURVES.with(|c| {
        c.borrow_mut()
            .entry(b.map(f32::to_bits))
            .or_insert_with(|| CAMediaTimingFunction::functionWithControlPoints(b[0], b[1], b[2], b[3]))
            .clone()
    })
}

/// A step is held this near (in progress) to its keyframe before jumping: a frame never lands inside it.
const STEP_HOLD: f64 = 1e-6;

/// The seam's keyframes (already the layer's values) played on `layer` at `path`, replacing what played there; its
/// last value left set once it's done.
fn play(layer: &CALayer, path: &str, a: &Animation<Retained<AnyObject>>) {
    let key = ns(path);
    let mut frames: Vec<(f64, Retained<AnyObject>, Easing)> = a.frames.iter().map(|(t, v, e)| (*t as f64, v.clone(), *e)).collect();
    // (no keyframe at the start: it starts from where it is)
    if frames.first().is_none_or(|f| f.0 > 0.) {
        let Some(now) = current(layer, path) else { return };
        frames.insert(0, (0., now, Easing::Linear));
    }
    if let Some(last) = frames.last().cloned().filter(|f| f.0 < 1.) {
        frames.push((1., last.1, Easing::Linear));
    }
    let mut values = vec![frames[0].1.clone()];
    let mut times = vec![NSNumber::new_f64(frames[0].0)];
    let mut curves = Vec::new();
    for pair in frames.windows(2) {
        let ((t0, v0, _), (t1, v1, e)) = (&pair[0], &pair[1]);
        if *e == Easing::Step {
            // (held, then a jump too short for any frame to show: Windows' step)
            let hold = (t1 - STEP_HOLD).max(*t0);
            values.push(v0.clone());
            times.push(NSNumber::new_f64(hold));
            curves.push(timing(Easing::Linear));
        }
        values.push(v1.clone());
        times.push(NSNumber::new_f64(*t1));
        curves.push(timing(*e));
    }
    let anim = CAKeyframeAnimation::animationWithKeyPath(Some(&key));
    unsafe { anim.setValues(Some(&NSArray::from_retained_slice(&values))) };
    anim.setKeyTimes(Some(&NSArray::from_retained_slice(&times)));
    anim.setTimingFunctions(Some(&NSArray::from_retained_slice(&curves)));
    anim.setDuration(a.duration_ms.max(1) as f64 / 1000.);
    if a.delay_ms > 0 {
        // (the first value held meanwhile)
        anim.setBeginTime(layer.convertTime_fromLayer(CACurrentMediaTime(), None) + a.delay_ms as f64 / 1000.);
        anim.setFillMode(unsafe { kCAFillModeBackwards });
    }
    if a.forever {
        anim.setRepeatCount(f32::INFINITY);
    }
    unsafe { layer.setValue_forKeyPath(Some(&values[values.len() - 1]), &key) };
    layer.addAnimation_forKey(&anim, Some(&key));
}

/// Each keyframe's value made the layer's.
fn mapped<V: Copy>(a: &Animation<V>, f: impl Fn(V) -> Retained<AnyObject>) -> Animation<Retained<AnyObject>> {
    Animation { frames: a.frames.iter().map(|(t, v, e)| (*t, f(*v), *e)).collect(), duration_ms: a.duration_ms, delay_ms: a.delay_ms, forever: a.forever }
}

/// The value at `path` as it shows now, made its set value, and whatever played there stopped.
fn stop_at(layer: &CALayer, path: &str) {
    if let Some(now) = current(layer, path) {
        set(layer, path, &now);
    }
}

impl seam::Layer for Visual {
    fn set_offset(&self, x: f32, y: f32) -> Result<()> {
        let place = &self.0.place;
        for path in ["position", "position.x", "position.y"] {
            place.removeAnimationForKey(&ns(path));
        }
        place.setPosition(CGPoint::new(pt(x as f64), pt(y as f64)));
        Ok(())
    }
    fn set_size(&self, w: f32, h: f32) -> Result<()> {
        self.0.st.borrow_mut().size = (w as f64, h as f64);
        self.0.lay_out();
        self.0.fit_parts();
        Ok(())
    }
    fn set_centre(&self, x: f32, y: f32) -> Result<()> {
        self.0.st.borrow_mut().centre = (x as f64, y as f64);
        self.0.lay_out();
        Ok(())
    }
    fn set_rotation(&self, degrees: f32) -> Result<()> {
        set(&self.0.body, "transform.rotation.z", &number((degrees as f64).to_radians()));
        Ok(())
    }
    fn set_opacity(&self, opacity: f32) -> Result<()> {
        set(&self.0.body, "opacity", &number(opacity as f64));
        Ok(())
    }
    fn set_visible(&self, visible: bool) -> Result<()> {
        self.0.place.setHidden(!visible);
        Ok(())
    }
    fn set_transform(&self, dx: f32, dy: f32, k: f32) -> Result<()> {
        let k = k as f64;
        self.0.place.setTransform(CATransform3D {
            m11: k, m12: 0., m13: 0., m14: 0.,
            m21: 0., m22: k, m23: 0., m24: 0.,
            m31: 0., m32: 0., m33: 1., m34: 0.,
            m41: pt(dx as f64), m42: pt(dy as f64), m43: 0., m44: 1.,
        });
        Ok(())
    }
    fn set_brush(&self, brush: Option<&Brush>) -> Result<()> {
        if !self.0.sprite {
            return Ok(());
        }
        let node = &self.0;
        let mut st = node.st.borrow_mut();
        if let Some(old) = st.content.take() {
            old.removeFromSuperlayer();
        }
        st.brush = brush.cloned();
        node.body.setBackgroundColor(None);
        let Some(brush) = brush else { return Ok(()) };
        if let BrushKind::Solid(c) = &*brush.0 {
            node.body.setBackgroundColor(Some(c));
            return Ok(());
        }
        let content = match &*brush.0 {
            BrushKind::Video(layer) => layer.clone(),
            _ => new_layer(),
        };
        content.setFrame(node.body.bounds());
        node.body.insertSublayer_atIndex(&content, 0);
        brush.show_on(&content, st.size)?;
        st.content = Some(content);
        Ok(())
    }
    fn stop(&self, prop: Scalar) -> Result<()> {
        for (layer, path) in targets(&self.0, prop) {
            stop_at(&layer, path);
            if prop == Scalar::OffsetX || prop == Scalar::OffsetY {
                // (an offset sliding as a pair stops where it is too)
                stop_at(&layer, "position");
            }
        }
        Ok(())
    }
    fn seek(&self, prop: Scalar, progress: f32) -> Result<()> {
        for (layer, path) in targets(&self.0, prop) {
            let key = ns(path);
            let Some(playing) = (unsafe { layer.animationForKey(&key) }) else { continue };
            // (the same animation from that far into its run, starting now)
            let copy: Retained<CAKeyframeAnimation> = unsafe { msg_send![&*playing, copy] };
            copy.setBeginTime(0.);
            copy.setTimeOffset(progress as f64 * copy.duration());
            layer.addAnimation_forKey(&copy, Some(&key));
        }
        Ok(())
    }
    fn insert_top(&self, child: &Visual) -> Result<()> {
        self.0.body.addSublayer(&child.0.place);
        Ok(())
    }
    fn insert_bottom(&self, child: &Visual) -> Result<()> {
        let above_brush = u32::from(self.0.st.borrow().content.is_some());
        self.0.body.insertSublayer_atIndex(&child.0.place, above_brush);
        Ok(())
    }
    fn insert_above(&self, child: &Visual, sibling: &Visual) -> Result<()> {
        self.0.body.insertSublayer_above(&child.0.place, Some(&sibling.0.place));
        Ok(())
    }
    fn remove(&self, child: &Visual) -> Result<()> {
        if child.0.place.superlayer().is_some_and(|p| Retained::as_ptr(&p) == Retained::as_ptr(&self.0.body)) {
            child.0.place.removeFromSuperlayer();
        }
        Ok(())
    }
    fn remove_all(&self) -> Result<()> {
        let content = self.0.st.borrow().content.clone();
        for layer in unsafe { self.0.body.sublayers() }.map(|s| s.to_vec()).unwrap_or_default() {
            if content.as_ref().is_none_or(|c| Retained::as_ptr(c) != Retained::as_ptr(&layer)) {
                layer.removeFromSuperlayer();
            }
        }
        Ok(())
    }
    fn has_children(&self) -> Result<bool> {
        let count = unsafe { self.0.body.sublayers() }.map_or(0, |s| s.count());
        Ok(count > usize::from(self.0.st.borrow().content.is_some()))
    }
}

// ---------- brushes ----------

/// Anything a sprite can show.
#[derive(Clone)]
pub struct Brush(Rc<BrushKind>);

enum BrushKind {
    Solid(CFRetained<CGColor>),
    Picture(Picture),
    /// A colour seen through a picture's alpha.
    Masked { colour: ColorBrush, mask: Picture },
    Blur { sigma: f64, saturation: f64 },
    /// The player's layer, drawn by mpv on its own thread.
    Video(Retained<CALayer>),
}

/// A drawn surface laid on a layer, maybe moving through it (an animated picture's frames).
struct Picture {
    surface: Surface,
    fit: Fit,
    /// The offset's animation, and whether it's playing.
    moving: RefCell<Option<Animation<[f32; 2]>>>,
    playing: Cell<bool>,
    /// The layers showing it, to move along with it.
    layers: RefCell<Vec<Weak<CALayer>>>,
}

impl Picture {
    /// What part of the surface a layer `size` (pixels) shows at `offset`: Core Animation's unit rectangle.
    fn part(&self, size: (f64, f64), offset: [f32; 2]) -> CGRect {
        let (pw, ph) = (self.surface.0.w as f64, self.surface.0.h as f64);
        match self.fit {
            Fit::Stretch => CGRect::new(CGPoint::new(0., 0.), CGSize::new(1., 1.)),
            Fit::At { scale, .. } => {
                let (sw, sh) = (pw * scale[0] as f64, ph * scale[1] as f64);
                CGRect::new(CGPoint::new(-offset[0] as f64 / sw, -offset[1] as f64 / sh), CGSize::new(size.0 / sw, size.1 / sh))
            }
        }
    }

    fn offset(&self) -> [f32; 2] {
        match self.fit {
            Fit::At { offset, .. } => offset,
            Fit::Stretch => [0., 0.],
        }
    }

    fn show_on(&self, layer: &CALayer, size: (f64, f64)) {
        layer.setContentsScale(window::scale());
        unsafe { layer.setContents(Some(&image_object(&self.surface.image()))) };
        layer.setContentsRect(self.part(size, self.offset()));
        self.surface.0.users.borrow_mut().push(Weak::from_retained(&layer.retain()));
        if let Some(a) = &*self.moving.borrow() {
            self.move_layer(layer, size, a);
        }
        self.layers.borrow_mut().retain(|l| l.load().is_some());
        self.layers.borrow_mut().push(Weak::from_retained(&layer.retain()));
    }

    /// Its offset's animation started on one layer, held or playing as the picture is.
    fn move_layer(&self, layer: &CALayer, size: (f64, f64), a: &Animation<[f32; 2]>) {
        play(layer, "contentsRect", &mapped(a, |o| rect_value(self.part(size, o))));
        hold(layer, !self.playing.get());
    }
}

/// A layer's own clock stopped where it is, or let go on from there.
fn hold(layer: &CALayer, still: bool) {
    let stopped = layer.speed() == 0.;
    if still && !stopped {
        let at = layer.convertTime_fromLayer(CACurrentMediaTime(), None);
        layer.setSpeed(0.);
        layer.setTimeOffset(at);
    } else if !still && stopped {
        let at = layer.timeOffset();
        layer.setSpeed(1.);
        layer.setTimeOffset(0.);
        layer.setBeginTime(0.);
        layer.setBeginTime(layer.convertTime_fromLayer(CACurrentMediaTime(), None) - at);
    }
}

impl Brush {
    /// Put on a sprite's brush layer, `size` pixels.
    fn show_on(&self, layer: &CALayer, size: (f64, f64)) -> Result<()> {
        match &*self.0 {
            BrushKind::Solid(c) => layer.setBackgroundColor(Some(c)),
            BrushKind::Picture(p) => p.show_on(layer, size),
            BrushKind::Masked { colour, mask } => {
                layer.setBackgroundColor(Some(&draw::colour(colour.0.argb.get())));
                colour.0.layers.borrow_mut().retain(|l| l.load().is_some());
                colour.0.layers.borrow_mut().push(Weak::from_retained(&layer.retain()));
                let m = new_layer();
                m.setFrame(layer.bounds());
                mask.show_on(&m, size);
                unsafe { layer.setMask(Some(&m)) };
            }
            BrushKind::Blur { sigma, saturation } => {
                // (behind it: held at its edges (no dark rim), saturated, then blurred)
                // On macOS a filter made by name has no inputs set (CIAffineClamp has no transform, so the chain gives
                // nothing) until setDefaults.
                let filter = |name: &str| {
                    let f = unsafe { CIFilter::filterWithName(&ns(name)) }.ok_or_else(|| Error::new(format!("{name} is missing")))?;
                    unsafe { f.setDefaults() };
                    Ok::<_, Error>(f)
                };
                let clamp = filter("CIAffineClamp")?;
                let vivid = filter("CIColorControls")?;
                unsafe { vivid.setValue_forKey(Some(&number(*saturation)), &ns("inputSaturation")) };
                let blur = filter("CIGaussianBlur")?;
                unsafe { blur.setValue_forKey(Some(&number(pt(*sigma))), &ns("inputRadius")) };
                let filters = NSArray::from_retained_slice(&[object(clamp), object(vivid), object(blur)]);
                unsafe { layer.setBackgroundFilters(Some(&filters)) };
                layer.setMasksToBounds(true);
            }
            BrushKind::Video(_) => {
                let bounds = layer.bounds().size;
                super::video::fit(layer, bounds.width, bounds.height, window::scale());
            }
        }
        Ok(())
    }

    /// Its layer fitted to a new size.
    fn fit(&self, layer: &CALayer, size: (f64, f64)) {
        let picture = match &*self.0 {
            BrushKind::Picture(p) => p,
            BrushKind::Masked { mask, .. } => {
                if let Some(m) = layer.mask() {
                    m.setFrame(layer.bounds());
                    m.setContentsRect(mask.part(size, mask.offset()));
                }
                return;
            }
            BrushKind::Video(_) => {
                let bounds = layer.bounds().size;
                super::video::fit(layer, bounds.width, bounds.height, window::scale());
                return;
            }
            _ => return,
        };
        if picture.moving.borrow().is_none() {
            layer.setContentsRect(picture.part(size, picture.offset()));
        }
    }
}

/// A colour that can glide to another (`animate_colour`).
#[derive(Clone)]
pub struct ColorBrush(Rc<Colour>);

struct Colour {
    argb: Cell<u32>,
    layers: RefCell<Vec<Weak<CALayer>>>,
}

// ---------- surfaces ----------

/// A surface drawn on by the CPU and shown as layer contents: a fresh image of it once a frame after it's painted.
#[derive(Clone)]
pub struct Surface(Rc<SurfaceInner>);

struct SurfaceInner {
    cg: CFRetained<CGContext>,
    w: u32,
    h: u32,
    image: RefCell<Option<CFRetained<CGImage>>>,
    /// The layers showing it, given the new image when it's painted again.
    users: RefCell<Vec<Weak<CALayer>>>,
}

thread_local! {
    /// Surfaces painted since their layers were last given their image.
    static PAINTED: RefCell<Vec<RcWeak<SurfaceInner>>> = const { RefCell::new(Vec::new()) };
}

impl Surface {
    /// Its image as painted last.
    fn image(&self) -> CFRetained<CGImage> {
        if let Some(i) = &*self.0.image.borrow() {
            return i.clone();
        }
        let image = CGBitmapContextCreateImage(Some(&self.0.cg)).expect("a surface's image");
        *self.0.image.borrow_mut() = Some(image.clone());
        image
    }

    fn painted(&self) {
        let old = self.0.image.borrow_mut().take();
        if old.is_some() {
            let first = PAINTED.with(|p| {
                let mut p = p.borrow_mut();
                p.push(Rc::downgrade(&self.0));
                p.len() == 1
            });
            if first {
                window::flush_soon();
            }
        }
    }
}

/// Every surface painted since the last time given to the layers that show it: once a frame, before Core Animation
/// takes the frame.
pub(super) fn flush() {
    let painted = PAINTED.with(|p| std::mem::take(&mut *p.borrow_mut()));
    for inner in painted.iter().filter_map(RcWeak::upgrade) {
        let surface = Surface(inner);
        let image = image_object(&surface.image());
        surface.0.users.borrow_mut().retain(|weak| {
            let Some(layer) = weak.load() else { return false };
            if unsafe { layer.contents() }.is_none() {
                return false;
            }
            unsafe { layer.setContents(Some(&image)) };
            true
        });
    }
}

// ---------- the compositor ----------

/// One path of an icon: its shape, whether it's filled, and its stroke width (0 for none).
type IconPath = (CFRetained<CGMutablePath>, bool, f32);

/// The layer tree in the window, and the text and drawing under it.
pub struct Graphics {
    root: Visual,
    text: Text,
    icons: RefCell<HashMap<&'static str, Rc<Vec<IconPath>>>>,
}

impl Graphics {
    pub(super) fn text(&self) -> &Text {
        &self.text
    }

    pub(super) fn icon_paths(&self, name: &'static str) -> Rc<Vec<IconPath>> {
        self.icons
            .borrow_mut()
            .entry(name)
            .or_insert_with(|| Rc::new(crate::icons::parts(name).iter().map(|part| (draw::trace(part.d, None), part.filled, part.stroke)).collect()))
            .clone()
    }

    fn visual(sprite: bool) -> Visual {
        let place = new_layer();
        place.setAnchorPoint(CGPoint::new(0., 0.));
        place.setBounds(CGRect::new(CGPoint::new(0., 0.), CGSize::new(0., 0.)));
        let body = new_layer();
        place.addSublayer(&body);
        let node = Node { place, body, sprite, st: RefCell::new(State::default()) };
        node.lay_out();
        Visual(Rc::new(node))
    }

    fn surface_brush_kind(&self, surface: &Surface, fit: Fit) -> Picture {
        Picture { surface: surface.clone(), fit, moving: RefCell::new(None), playing: Cell::new(true), layers: Default::default() }
    }

    /// Painted at `scale` into a part of a surface (pixels), cleared first.
    fn draw_in(&self, surface: &Surface, area: CGRect, clip: Option<CGRect>, scale: f32, draw: impl FnOnce(&mut Canvas<'_>) -> Result<()>) -> Result<bool> {
        let cg = &surface.0.cg;
        let c = Some(&**cg);
        CGContext::save_g_state(c);
        // (y running down, as the app draws: the first row of the surface its top)
        CGContext::translate_ctm(c, 0., surface.0.h as f64);
        CGContext::scale_ctm(c, 1., -1.);
        CGContext::clear_rect(c, area);
        if let Some(clip) = clip {
            CGContext::clip_to_rect(c, clip);
            CGContext::translate_ctm(c, clip.origin.x, clip.origin.y);
        }
        CGContext::scale_ctm(c, scale as f64, scale as f64);
        CGContext::set_should_antialias(c, true);
        CGContext::set_allows_font_smoothing(c, false);
        CGContext::set_should_subpixel_position_fonts(c, true);
        CGContext::set_should_subpixel_quantize_fonts(c, false);
        let result = draw(&mut Canvas { cg, g: self, scale });
        CGContext::restore_g_state(c);
        surface.painted();
        result.map(|_| true)
    }
}

impl seam::Compositor for Graphics {
    fn new(window: &Window, fonts: &Path) -> Result<Graphics> {
        let root = Graphics::visual(false);
        window.host(&root.0.place);
        Ok(Graphics { root, text: Text::new(fonts)?, icons: Default::default() })
    }

    fn root(&self) -> &Visual {
        &self.root
    }

    fn sprite(&self) -> Result<Visual> {
        Ok(Graphics::visual(true))
    }

    fn container(&self) -> Result<Visual> {
        Ok(Graphics::visual(false))
    }

    fn clip(&self, visual: &Visual, clip: Clip) -> Result<()> {
        let node = &visual.0;
        let body = &node.body;
        unsafe { body.setMask(None) };
        body.setMasksToBounds(false);
        body.setCornerRadius(0.);
        node.st.borrow_mut().inset = None;
        match clip {
            Clip::None => {}
            Clip::Rounded { radius, .. } => {
                body.setCornerRadius(pt(radius as f64));
                body.setMasksToBounds(true);
            }
            Clip::Inset => {
                let mask = new_layer();
                mask.setAnchorPoint(CGPoint::new(0., 0.));
                mask.setPosition(CGPoint::new(0., 0.));
                mask.setMasksToBounds(true);
                let keep = new_layer();
                keep.setAnchorPoint(CGPoint::new(0., 0.));
                keep.setBackgroundColor(Some(&draw::colour(0xFFFFFFFF)));
                mask.addSublayer(&keep);
                unsafe { body.setMask(Some(&mask)) };
                node.st.borrow_mut().inset = Some(Inset { mask, keep, right: 0. });
                node.fit_parts();
            }
        }
        Ok(())
    }

    fn solid(&self, argb: u32) -> Result<Brush> {
        Ok(Brush(Rc::new(BrushKind::Solid(draw::colour(argb)))))
    }

    fn color_brush(&self, argb: u32) -> Result<ColorBrush> {
        Ok(ColorBrush(Rc::new(Colour { argb: Cell::new(argb), layers: Default::default() })))
    }

    fn surface_brush(&self, surface: &Surface, fit: Fit) -> Result<Brush> {
        Ok(Brush(Rc::new(BrushKind::Picture(self.surface_brush_kind(surface, fit)))))
    }

    fn masked(&self, colour: &ColorBrush, mask: &Brush) -> Result<Brush> {
        let BrushKind::Picture(p) = &*mask.0 else { return Err(Error::new("a mask is a drawn surface")) };
        let mask = Picture { surface: p.surface.clone(), fit: p.fit, moving: RefCell::new(p.moving.borrow().clone()), playing: Cell::new(p.playing.get()), layers: Default::default() };
        Ok(Brush(Rc::new(BrushKind::Masked { colour: colour.clone(), mask })))
    }

    fn backdrop_blur(&self, sigma: f32, saturation: f32) -> Result<Brush> {
        Ok(Brush(Rc::new(BrushKind::Blur { sigma: sigma as f64, saturation: saturation as f64 })))
    }

    fn video_brush(&self, video: super::video::Surface) -> Result<Brush> {
        Ok(Brush(Rc::new(BrushKind::Video(video.into_layer()))))
    }

    fn animate(&self, visual: &Visual, prop: Scalar, a: &Animation<f32>) -> Result<()> {
        let node = &visual.0;
        match prop {
            Scalar::OffsetX | Scalar::OffsetY => {
                let path = if prop == Scalar::OffsetX { "position.x" } else { "position.y" };
                play(&node.place, path, &mapped(a, |v| number(pt(v as f64))));
            }
            Scalar::Opacity => play(&node.body, "opacity", &mapped(a, |v| number(v as f64))),
            Scalar::Rotation => play(&node.body, "transform.rotation.z", &mapped(a, |v| number((v as f64).to_radians()))),
            Scalar::ClipLeft => {
                let st = node.st.borrow();
                if let Some(inset) = &st.inset {
                    let values = mapped(a, |v| number(pt(v as f64)));
                    play(&inset.mask, "position.x", &values);
                    play(&inset.mask, "bounds.origin.x", &values);
                }
            }
            Scalar::ClipRight => {
                let mut st = node.st.borrow_mut();
                let w = st.size.0;
                if let Some(inset) = st.inset.as_mut() {
                    play(&inset.keep, "bounds.size.width", &mapped(a, |v| number(pt((w - v as f64).max(0.)))));
                    if let Some(last) = a.frames.last() {
                        inset.right = last.1 as f64;
                    }
                }
            }
        }
        Ok(())
    }

    fn animate_pair(&self, visual: &Visual, prop: Pair, a: &Animation<[f32; 2]>) -> Result<()> {
        let node = &visual.0;
        match prop {
            Pair::Offset => play(&node.place, "position", &mapped(a, |[x, y]| point(pt(x as f64), pt(y as f64)))),
            Pair::Scale => {
                play(&node.body, "transform.scale.x", &mapped(a, |[x, _]| number(x as f64)));
                play(&node.body, "transform.scale.y", &mapped(a, |[_, y]| number(y as f64)));
            }
        }
        Ok(())
    }

    fn animate_colour(&self, brush: &ColorBrush, a: &Animation<u32>) -> Result<()> {
        if let Some(last) = a.frames.last() {
            brush.0.argb.set(last.1);
        }
        let values = mapped(a, |c| cg_object(&draw::colour(c)));
        brush.0.layers.borrow_mut().retain(|weak| {
            let Some(layer) = weak.load() else { return false };
            play(&layer, "backgroundColor", &values);
            true
        });
        Ok(())
    }

    fn animate_brush_offset(&self, brush: &Brush, a: &Animation<[f32; 2]>) -> Result<()> {
        let BrushKind::Picture(p) = &*brush.0 else { return Ok(()) };
        *p.moving.borrow_mut() = Some(a.clone());
        for layer in p.layers.borrow().iter().filter_map(Weak::load) {
            let size = (layer.bounds().size.width * window::scale(), layer.bounds().size.height * window::scale());
            p.move_layer(&layer, size, a);
        }
        Ok(())
    }

    fn play_brush(&self, brush: &Brush, playing: bool) -> Result<bool> {
        let BrushKind::Picture(p) = &*brush.0 else { return Ok(false) };
        if p.moving.borrow().is_none() {
            return Ok(false);
        }
        p.playing.set(playing);
        for layer in p.layers.borrow().iter().filter_map(Weak::load) {
            hold(&layer, !playing);
        }
        Ok(true)
    }

    fn surface(&self, pw: u32, ph: u32) -> Result<Surface> {
        let info = objc2_core_graphics::CGImageAlphaInfo::PremultipliedFirst.0 | objc2_core_graphics::CGImageByteOrderInfo::Order32Little.0;
        let cg = unsafe { CGBitmapContextCreate(std::ptr::null_mut(), pw as usize, ph as usize, 8, 0, Some(&draw::srgb()), info) }
            .ok_or_else(|| Error::new(format!("no {pw}x{ph} surface")))?;
        Ok(Surface(Rc::new(SurfaceInner { cg, w: pw, h: ph, image: RefCell::new(None), users: Default::default() })))
    }

    fn paint(&self, surface: &Surface, scale: f32, draw: impl FnOnce(&mut Canvas<'_>) -> Result<()>) -> Result<bool> {
        let whole = CGRect::new(CGPoint::new(0., 0.), CGSize::new(surface.0.w as f64, surface.0.h as f64));
        self.draw_in(surface, whole, None, scale, draw)
    }

    fn paint_in(&self, surface: &Surface, x: u32, y: u32, w: u32, h: u32, scale: f32, draw: impl FnOnce(&mut Canvas<'_>) -> Result<()>) -> Result<bool> {
        // (the part and the pixel all round it cleared: a skin shown a touch smaller or bigger samples past its edge)
        let (lx, ty) = (x.min(1), y.min(1));
        let area = CGRect::new(CGPoint::new((x - lx) as f64, (y - ty) as f64), CGSize::new((w + 1 + lx) as f64, (h + 1 + ty) as f64));
        let part = CGRect::new(CGPoint::new(x as f64, y as f64), CGSize::new(w as f64, h as f64));
        self.draw_in(surface, area, Some(part), scale, draw)
    }

    fn replace_device(&mut self) -> Result<()> {
        Ok(())
    }

    fn text_size(&self, text: &str, style: &TextStyle, width: f32) -> Result<(f32, f32)> {
        let layout = self.text.layout(text, style, width, 1e6);
        Ok((layout.width as f32, layout.height as f32))
    }

    fn caret_x(&self, text: &str, style: &TextStyle, at: usize) -> Result<f32> {
        Ok(self.text.caret_x(text, style, at) as f32)
    }

    fn hit_char(&self, text: &str, style: &TextStyle, x: f32) -> Result<usize> {
        Ok(self.text.hit_char(text, style, x))
    }
}
