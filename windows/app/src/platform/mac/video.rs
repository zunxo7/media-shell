//! libmpv's picture on the Mac: mpv's OpenGL render API draws into a `CAOpenGLLayer` from a thread of its own, as
//! IINA embeds mpv (docs/plans/mac-video.md). The compositor shows the layer under the app's visuals.
//!
//! Every `mpv_render_*` call is made on the video thread: it makes the render context, draws when Core Animation
//! calls the layer back inside the thread's own `display`, consumes frames nobody saw and frees the context. Other
//! threads only wake it. It never calls the rest of libmpv, since with advanced control a wait on the player thread would
//! freeze mpv's core for good (render.h).
// CAOpenGLLayer and CGL are deprecated since macOS 10.14 but still ship, and mpv has no Metal render API.
#![allow(deprecated)]

use crate::mpv::{Mpv, RenderContext, RenderParam};
use crate::platform::{Error, Result};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_core_foundation::{CFTimeInterval, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{kCGColorSpaceSRGB, CGColor, CGColorSpace};
use objc2_core_video::CVTimeStamp;
use objc2_open_gl::{
    CGLChoosePixelFormat, CGLContextObj, CGLContextParameter, CGLCreateContext, CGLError, CGLLockContext,
    CGLOpenGLProfile, CGLPixelFormatAttribute, CGLPixelFormatObj, CGLReleaseContext, CGLReleasePixelFormat,
    CGLRetainContext, CGLRetainPixelFormat, CGLSetCurrentContext, CGLSetParameter, CGLUnlockContext,
};
use objc2_quartz_core::{kCAGravityResizeAspectFill, CALayer, CAOpenGLLayer, CATransaction};
use std::ffi::{c_char, c_int, c_void};
use std::ptr::{null_mut, NonNull};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle, ThreadId};

/// libmpv's file, in the app bundle's Frameworks.
pub const LIBRARY: &str = "libmpv.2.dylib";

/// The options that make mpv draw through the render API, set before the rest. `gpu-hwdec-interop=auto` loads every
/// interop up front for vo=libmpv, VideoToolbox's zero-copy one among them.
pub const OPTIONS: &[(&str, &str)] = &[("vo", "libmpv"), ("gpu-hwdec-interop", "auto")];

// mpv_render_param_type and mpv_render_update_flag (render.h).
const API_TYPE: c_int = 1;
const OPENGL_INIT_PARAMS: c_int = 2;
const OPENGL_FBO: c_int = 3;
const FLIP_Y: c_int = 4;
const DEPTH: c_int = 5;
const ADVANCED_CONTROL: c_int = 10;
const SKIP_RENDERING: c_int = 13;
const UPDATE_FRAME: u64 = 1;

const GL_VIEWPORT: u32 = 0x0BA2;
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
const RTLD_LAZY: c_int = 0x1;
const RTLD_LOCAL: c_int = 0x4;

#[link(name = "OpenGL", kind = "framework")]
extern "C" {
    fn glGetIntegerv(pname: u32, data: *mut i32);
    fn glFlush();
    fn glClearColor(r: f32, g: f32, b: f32, a: f32);
    fn glClear(mask: u32);
}

extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

#[repr(C)]
struct OpenGlInit {
    get_proc_address: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
struct Fbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

fn param<T>(kind: c_int, value: &mut T) -> RenderParam {
    RenderParam { kind, data: (value as *mut T).cast() }
}

const END: RenderParam = RenderParam { kind: 0, data: null_mut() };

/// The picture's producer, on the player thread: the video thread and its layer. Dropped before the `Mpv` it was
/// made from, it frees the render context first, as render.h requires.
pub struct Output {
    render: Arc<Render>,
    thread: Option<JoinHandle<()>>,
    layer: Option<Surface>,
}

impl Output {
    /// After mpv_initialize, before the first loadfile (vo=libmpv can't start video without a render context).
    pub fn new(mpv: &Mpv, _w: i32, _h: i32) -> Result<Output> {
        let render = Arc::new(Render::new(mpv)?);
        let (ready, made) = mpsc::sync_channel(1);
        let handle = mpv.h as usize;
        let thread = thread::Builder::new()
            .name("video".into())
            .spawn({
                let render = render.clone();
                move || run(&render, handle, ready)
            })
            .map_err(|e| Error::new(format!("Couldn't start the video thread. {e}")))?;
        match made.recv() {
            Ok(Ok(layer)) => Ok(Output { render, thread: Some(thread), layer: Some(layer) }),
            Ok(Err(code)) => {
                let _ = thread.join();
                Err(Error::new(format!("Couldn't start mpv's renderer. {}", mpv.err(code))))
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::new("The video thread stopped."))
            }
        }
    }

    /// Nothing to do: the layer's own size is the picture's.
    pub fn resize(&mut self, _mpv: &Mpv, _w: i32, _h: i32) {}

    /// The layer, the first time it's asked for.
    pub fn surface(&mut self, _mpv: &Mpv, _due: bool) -> Option<Surface> {
        self.layer.take()
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.render.signal(|s| s.stop = true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The video layer, on its way from the player thread to the compositor.
pub struct Surface(Retained<VideoLayer>);

// SAFETY: the layer is only changed inside explicit CATransactions, which Core Animation allows from any thread.
unsafe impl Send for Surface {}

impl Surface {
    /// The layer, for the compositor to put under the video visual's content (its first sublayer) and size with
    /// [`fit`].
    pub(super) fn into_layer(self) -> Retained<CALayer> {
        Retained::into_super(Retained::into_super(self.0))
    }
}

/// The video layer laid over its visual: `w` by `h` points, at `scale` pixels a point. The scale is the window's
/// backing scale, which a hand-made sublayer doesn't inherit. Without animation, as every layer in the tree changes.
pub(super) fn fit(layer: &CALayer, w: f64, h: f64, scale: f64) {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    layer.setFrame(CGRect::new(CGPoint::new(0., 0.), CGSize::new(w as CGFloat, h as CGFloat)));
    layer.setContentsScale(scale as CGFloat);
    CATransaction::commit();
}

/// The render API's functions, copied from mpv's `Api` so the layer can hold them without the `Mpv`.
#[derive(Clone, Copy)]
struct Fns {
    create: unsafe extern "C" fn(*mut RenderContext, *mut c_void, *mut RenderParam) -> c_int,
    set_update_callback: unsafe extern "C" fn(RenderContext, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void),
    update: unsafe extern "C" fn(RenderContext) -> u64,
    render: unsafe extern "C" fn(RenderContext, *mut RenderParam) -> c_int,
    free: unsafe extern "C" fn(RenderContext),
}

#[derive(Default)]
struct Wake {
    /// mpv has a frame, or the picture must be drawn again.
    frame: bool,
    stop: bool,
}

/// What the layer and the video thread share.
struct Render {
    cgl: CGLContextObj,
    format: CGLPixelFormatObj,
    /// mpv's render context: null until the video thread makes it and once it's freed. The functions are only
    /// called while it's set, so never after the library is gone.
    context: AtomicPtr<c_void>,
    fns: Fns,
    thread: OnceLock<ThreadId>,
    state: Mutex<Wake>,
    wake: Condvar,
    /// Draw the current frame again (the layer was resized or shown) even if mpv has no new one.
    force: AtomicBool,
    /// Core Animation called `draw` during the last `display`.
    drew: AtomicBool,
    /// The last non-zero framebuffer Core Animation drew in: it owns framebuffer 0 but can hand it to us.
    fbo: AtomicI32,
}

// SAFETY: the CGL objects are used under CGLLockContext; the rest is atomics and a mutex.
unsafe impl Send for Render {}
unsafe impl Sync for Render {}

impl Drop for Render {
    fn drop(&mut self) {
        unsafe {
            CGLReleaseContext(self.cgl);
            CGLReleasePixelFormat(self.format);
        }
    }
}

impl Render {
    /// IINA's pixel format (OpenGL 3.2 core, needed by VideoToolbox's zero-copy interop), dropping the optional
    /// attributes from the end until the system takes it, and a context on it. Accelerated goes last: a Mac in a
    /// virtual machine has no accelerated OpenGL, only Apple's software renderer.
    fn new(mpv: &Mpv) -> Result<Render> {
        let required = [
            CGLPixelFormatAttribute::CGLPFAOpenGLProfile,
            CGLPixelFormatAttribute(CGLOpenGLProfile::CGLOGLPVersion_3_2_Core.0),
            CGLPixelFormatAttribute::CGLPFADoubleBuffer,
        ];
        let optional = [
            CGLPixelFormatAttribute::CGLPFAAccelerated,
            CGLPixelFormatAttribute::CGLPFAAllowOfflineRenderers,
            CGLPixelFormatAttribute::CGLPFASupportsAutomaticGraphicsSwitching,
        ];
        let format = (0..=optional.len())
            .rev()
            .find_map(|n| {
                let mut attributes: Vec<_> = required.iter().chain(&optional[..n]).copied().collect();
                attributes.push(CGLPixelFormatAttribute(0));
                let (mut format, mut count) = (null_mut(), 0);
                let error = unsafe {
                    CGLChoosePixelFormat(
                        NonNull::new_unchecked(attributes.as_mut_ptr()),
                        NonNull::from(&mut format),
                        NonNull::from(&mut count),
                    )
                };
                (error == CGLError::NoError && !format.is_null()).then_some(format)
            })
            .ok_or_else(|| Error::new("No OpenGL pixel format for the video."))?;
        let mut cgl = null_mut();
        let error = unsafe { CGLCreateContext(format, null_mut(), NonNull::from(&mut cgl)) };
        if error != CGLError::NoError || cgl.is_null() {
            unsafe { CGLReleasePixelFormat(format) };
            return Err(Error::new(format!("Couldn't make the video's OpenGL context ({}).", error.0)));
        }
        let interval = 1;
        unsafe { CGLSetParameter(cgl, CGLContextParameter::CGLCPSwapInterval, NonNull::from(&interval)) };
        let api = &mpv.api;
        Ok(Render {
            cgl,
            format,
            context: AtomicPtr::new(null_mut()),
            fns: Fns {
                create: api.mpv_render_context_create,
                set_update_callback: api.mpv_render_context_set_update_callback,
                update: api.mpv_render_context_update,
                render: api.mpv_render_context_render,
                free: api.mpv_render_context_free,
            },
            thread: OnceLock::new(),
            state: Mutex::default(),
            wake: Condvar::new(),
            force: AtomicBool::new(false),
            drew: AtomicBool::new(false),
            fbo: AtomicI32::new(0),
        })
    }

    fn signal(&self, f: impl FnOnce(&mut Wake)) {
        f(&mut self.state.lock().unwrap());
        self.wake.notify_one();
    }

    /// The video thread asleep until there's something to draw; false once it's to stop.
    fn wait(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        while !state.frame && !state.stop {
            state = self.wake.wait(state).unwrap();
        }
        state.frame = false;
        !state.stop
    }

    fn on_video_thread(&self) -> bool {
        self.thread.get() == Some(&thread::current().id())
    }

    /// `f` with the GL context locked and current on this thread, as every render call outside `draw` needs.
    fn with_context<T>(&self, f: impl FnOnce() -> T) -> T {
        unsafe {
            CGLLockContext(self.cgl);
            CGLSetCurrentContext(self.cgl);
        }
        let out = f();
        unsafe {
            CGLSetCurrentContext(null_mut());
            CGLUnlockContext(self.cgl);
        }
        out
    }

    /// mpv's render context on this GL context, its update callback set at once: with advanced control the core can
    /// wait for it. mpv's error code if it couldn't be made.
    fn create(&self, mpv: usize) -> std::result::Result<(), c_int> {
        let mut init = OpenGlInit { get_proc_address: gl_address, get_proc_address_ctx: null_mut() };
        let mut advanced: c_int = 1;
        let mut params = [
            RenderParam { kind: API_TYPE, data: c"opengl".as_ptr().cast_mut().cast() },
            param(OPENGL_INIT_PARAMS, &mut init),
            param(ADVANCED_CONTROL, &mut advanced),
            END,
        ];
        let mut context = null_mut();
        let made = unsafe { (self.fns.create)(&mut context, mpv as *mut c_void, params.as_mut_ptr()) };
        if made < 0 {
            return Err(made);
        }
        unsafe { (self.fns.set_update_callback)(context, Some(on_update), self as *const Render as *mut c_void) };
        self.context.store(context, Ordering::Release);
        Ok(())
    }

    /// Whether `draw` should be called now. Another thread's display, such as the main thread's after a resize, is
    /// handed to the video thread instead, so the main thread never waits on mpv.
    fn can_draw(&self) -> bool {
        if !self.on_video_thread() {
            self.force.store(true, Ordering::Relaxed);
            self.signal(|s| s.frame = true);
            return false;
        }
        let context = self.context.load(Ordering::Acquire);
        !context.is_null()
            && (self.force.load(Ordering::Relaxed) || unsafe { (self.fns.update)(context) } & UPDATE_FRAME != 0)
    }

    /// A frame Core Animation didn't draw (the window hidden, minimised or on another Space) consumed unseen:
    /// otherwise mpv waits for a render that never comes.
    fn skip(&self) {
        let context = self.context.load(Ordering::Acquire);
        if context.is_null() || unsafe { (self.fns.update)(context) } & UPDATE_FRAME == 0 {
            return;
        }
        let mut skip: c_int = 1;
        let mut params = [param(SKIP_RENDERING, &mut skip), END];
        unsafe { (self.fns.render)(context, params.as_mut_ptr()) };
    }

    /// The render context gone: no more callbacks, and the layer draws black from here on.
    fn free(&self) {
        let context = self.context.swap(null_mut(), Ordering::AcqRel);
        if !context.is_null() {
            unsafe {
                (self.fns.set_update_callback)(context, None, null_mut());
                (self.fns.free)(context);
            }
        }
    }
}

/// mpv's word that a frame is due, on one of its own threads: only the video thread is woken.
unsafe extern "C" fn on_update(render: *mut c_void) {
    let render = unsafe { &*(render as *const Render) };
    render.signal(|s| s.frame = true);
}

/// OpenGL's functions for mpv, looked up as IINA and mpv do.
unsafe extern "C" fn gl_address(_: *mut c_void, name: *const c_char) -> *mut c_void {
    static GL: OnceLock<usize> = OnceLock::new();
    let gl = *GL.get_or_init(|| unsafe {
        dlopen(c"/System/Library/Frameworks/OpenGL.framework/OpenGL".as_ptr(), RTLD_LAZY | RTLD_LOCAL) as usize
    });
    if gl == 0 {
        return null_mut();
    }
    unsafe { dlsym(gl as *mut c_void, name) }
}

/// The video thread: makes the render context and the layer, then draws each frame as mpv asks.
fn run(render: &Arc<Render>, mpv: usize, ready: mpsc::SyncSender<std::result::Result<Surface, c_int>>) {
    let _ = render.thread.set(thread::current().id());
    if let Err(code) = render.with_context(|| render.create(mpv)) {
        let _ = ready.send(Err(code));
        return;
    }
    let layer = VideoLayer::new(render.clone());
    if ready.send(Ok(Surface(layer.clone()))).is_ok() {
        while render.wait() {
            render.drew.store(false, Ordering::Relaxed);
            // Off the main thread, Core Animation needs an explicit transaction, and the flush commits one nested
            // in an implicit transaction (IINA #5038, #5029).
            CATransaction::begin();
            layer.display();
            CATransaction::commit();
            CATransaction::flush();
            if !render.drew.load(Ordering::Relaxed) {
                render.with_context(|| render.skip());
            }
        }
    }
    render.with_context(|| render.free());
}

define_class!(
    // SAFETY: CAOpenGLLayer is made to be subclassed for exactly these overrides, and VideoLayer has no Drop.
    #[unsafe(super(CAOpenGLLayer, CALayer, NSObject))]
    #[name = "ZFVideoLayer"]
    #[ivars = Arc<Render>]
    struct VideoLayer;

    impl VideoLayer {
        #[unsafe(method(copyCGLPixelFormatForDisplayMask:))]
        fn copy_pixel_format(&self, _mask: u32) -> CGLPixelFormatObj {
            unsafe { CGLRetainPixelFormat(self.ivars().format) }
        }

        /// The one context mpv's renderer was made on; "copy" hands Core Animation a reference it releases.
        #[unsafe(method(copyCGLContextForPixelFormat:))]
        fn copy_context(&self, _format: CGLPixelFormatObj) -> CGLContextObj {
            unsafe { CGLRetainContext(self.ivars().cgl) }
        }

        #[unsafe(method(canDrawInCGLContext:pixelFormat:forLayerTime:displayTime:))]
        fn can_draw(&self, _cgl: CGLContextObj, _format: CGLPixelFormatObj, _t: CFTimeInterval, _ts: *const CVTimeStamp) -> bool {
            self.ivars().can_draw()
        }

        #[unsafe(method(drawInCGLContext:pixelFormat:forLayerTime:displayTime:))]
        fn draw(&self, _cgl: CGLContextObj, _format: CGLPixelFormatObj, _t: CFTimeInterval, _ts: *const CVTimeStamp) {
            let render = self.ivars();
            let context = render.context.load(Ordering::Acquire);
            if context.is_null() || !render.on_video_thread() {
                unsafe {
                    glClearColor(0., 0., 0., 1.);
                    glClear(GL_COLOR_BUFFER_BIT);
                    glFlush();
                }
                return;
            }
            let mut fbo = 0;
            let mut viewport = [0; 4];
            unsafe {
                glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut fbo);
                glGetIntegerv(GL_VIEWPORT, viewport.as_mut_ptr());
            }
            if fbo != 0 {
                render.fbo.store(fbo, Ordering::Relaxed);
            } else {
                fbo = render.fbo.load(Ordering::Relaxed);
            }
            let (mut w, mut h) = (viewport[2], viewport[3]);
            if w <= 0 || h <= 0 {
                let (size, scale) = (self.bounds().size, self.contentsScale());
                (w, h) = ((size.width * scale) as c_int, (size.height * scale) as c_int);
            }
            if w <= 0 || h <= 0 {
                return;
            }
            let mut target = Fbo { fbo, w, h, internal_format: 0 };
            let (mut flip, mut depth): (c_int, c_int) = (1, 8);
            let mut params = [param(OPENGL_FBO, &mut target), param(FLIP_Y, &mut flip), param(DEPTH, &mut depth), END];
            unsafe {
                (render.fns.render)(context, params.as_mut_ptr());
                glFlush();
            }
            render.force.store(false, Ordering::Relaxed);
            render.drew.store(true, Ordering::Relaxed);
        }

        /// Core Animation's shadow copy (made when the backing scale changes) shares the original's renderer.
        #[unsafe(method_id(initWithLayer:))]
        fn init_with_layer(this: Allocated<Self>, layer: &AnyObject) -> Retained<Self> {
            let render = layer.downcast_ref::<VideoLayer>().expect("a copy of a video layer").ivars().clone();
            let this = this.set_ivars(render);
            unsafe { msg_send![super(this), initWithLayer: layer] }
        }
    }
);

impl VideoLayer {
    /// Opaque black until the first frame, colour-matched from sRGB to the display, keeping the last frame's shape
    /// while it's resized; drawn whenever the video thread asks, never on Core Animation's own clock.
    fn new(render: Arc<Render>) -> Retained<VideoLayer> {
        let this = Self::alloc().set_ivars(render);
        let layer: Retained<VideoLayer> = unsafe { msg_send![super(this), init] };
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        layer.setAsynchronous(false);
        layer.setOpaque(true);
        layer.setBackgroundColor(Some(&CGColor::new_generic_gray(0., 1.)));
        layer.setContentsGravity(unsafe { kCAGravityResizeAspectFill });
        layer.setNeedsDisplayOnBoundsChange(true);
        layer.setColorspace(unsafe { CGColorSpace::with_name(Some(kCGColorSpaceSRGB)) }.as_deref());
        CATransaction::commit();
        layer
    }
}
