//! The app's AppKit window: the Mac's own title bar (its three buttons) laid transparent over the app's top strip,
//! a layer-hosting view filling it, and the view's events turned into the seam's. Shortcuts held with Cmd come as
//! Windows' Ctrl ones (`ctrl_down`); keys come as Windows' key codes.
use crate::platform::{key, Callbacks, Event, Opening, Result, Shell, Wake, WindowCommand};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy,
    NSApplicationDelegate, NSApplicationTerminateReply, NSBackingStoreType, NSColor, NSEvent, NSEventModifierFlags, NSEventType, NSImage, NSMenu,
    NSMenuItem, NSOpenPanel, NSPasteboard, NSPasteboardTypeString, NSResponder, NSRunningApplication, NSTextInputClient, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSWindow, NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowDelegate, NSWindowStyleMask,
    NSWindowTitleVisibility,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSBundle, NSNotFound, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRange, NSRangePointer, NSRect,
    NSSize, NSString, NSUInteger, NSUserDefaults,
};
use objc2_quartz_core::CALayer;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Opened without being shown: it shows once the update check has answered.
static HIDDEN_START: AtomicBool = AtomicBool::new(false);
/// The setup's and the bootstrapper's size (layout units).
static SMALL: OnceLock<(f32, f32)> = OnceLock::new();

thread_local! {
    static CALLBACKS: Cell<Option<Callbacks>> = const { Cell::new(None) };
    static WINDOW: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
    static VIEW: RefCell<Option<Retained<View>>> = const { RefCell::new(None) };
    static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
    /// Points to pixels: the window's backing scale.
    static SCALE: Cell<f64> = const { Cell::new(1.) };
    /// Events being handled inside one another (a resize inside a click): only the outermost draws.
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    /// An Edit menu item is being carried out: it counts as Cmd held, as when its shortcut is pressed.
    pub(super) static MENU_CMD: Cell<bool> = const { Cell::new(false) };
    /// The frame timer: running, and which start this is (a stopped one's late firing is ignored).
    static FRAME: Cell<(bool, u64)> = const { Cell::new((false, 0)) };
}

/// The window's backing scale (pixels per point).
pub(super) fn scale() -> f64 {
    SCALE.get()
}

fn window() -> Option<Retained<NSWindow>> {
    WINDOW.with(|w| w.borrow().clone())
}

/// The view's layer kept the view's size (a hosted layer is the app's to size), so the app's top left stays the
/// window's.
fn fit_host() {
    VIEW.with(|v| {
        if let Some(view) = v.borrow().as_ref() {
            if let Some(host) = view.layer() {
                host.setFrame(view.bounds());
            }
        }
    });
}

/// An event to the app; then, after the outermost one, what changed drawn and its pictures handed to their layers.
fn deliver(event: Event) -> bool {
    let Some(c) = CALLBACKS.get() else { return false };
    DEPTH.set(DEPTH.get() + 1);
    let handled = (c.event)(event);
    if DEPTH.get() == 1 {
        (c.idle)();
        super::compositor::flush();
    }
    DEPTH.set(DEPTH.get() - 1);
    handled
}

// ---------- the main queue ----------

#[repr(C)]
struct DispatchQueue([u8; 0]);

extern "C" {
    static _dispatch_main_q: DispatchQueue;
    fn dispatch_async_f(queue: *const DispatchQueue, context: *mut c_void, work: extern "C" fn(*mut c_void));
    fn dispatch_after_f(when: u64, queue: *const DispatchQueue, context: *mut c_void, work: extern "C" fn(*mut c_void));
    fn dispatch_time(when: u64, delta: i64) -> u64;
}

/// `work` run on the UI thread's queue after `ms`, given `context`.
fn after(ms: f64, context: usize, work: extern "C" fn(*mut c_void)) {
    unsafe {
        let when = dispatch_time(0, (ms * 1e6) as i64);
        dispatch_after_f(when, &_dispatch_main_q, context as *mut c_void, work);
    }
}

extern "C" fn flushed(_: *mut c_void) {
    super::compositor::flush();
}

/// Painted surfaces handed to their layers on the UI thread's next turn, before Core Animation takes the frame:
/// for painting done outside an event (the first frames, drawn before the window's loop runs).
pub(super) fn flush_soon() {
    unsafe { dispatch_async_f(&_dispatch_main_q, std::ptr::null_mut(), flushed) };
}

extern "C" fn woken(_: *mut c_void) {
    deliver(Event::Wake);
}

extern "C" fn frame_due(start: *mut c_void) {
    let (running, current) = FRAME.get();
    if !running || current != start as u64 {
        return;
    }
    // (as Windows' timer, it comes again until it's stopped)
    after(frame_ms(), start as usize, frame_due);
    deliver(Event::Frame);
}

extern "C" fn tick(_: *mut c_void) {
    after(200., 0, tick);
    deliver(Event::Tick);
}

extern "C" fn close_now(_: *mut c_void) {
    if let Some(w) = window() {
        w.performClose(None);
    }
}

extern "C" fn into_bootstrap(_: *mut c_void) {
    HIDDEN_START.store(false, Ordering::Relaxed);
    let Some(w) = window() else { return };
    let (bw, bh) = SMALL.get().copied().unwrap_or_default();
    if w.isZoomed() {
        w.zoom(None);
    }
    if let Some(screen) = w.screen() {
        let area = screen.visibleFrame();
        let (bw, bh) = (bw as f64, bh as f64);
        let origin = NSPoint::new(area.origin.x + (area.size.width - bw) / 2., area.origin.y + (area.size.height - bh) / 2.);
        w.setFrame_display(NSRect::new(origin, NSSize::new(bw, bh)), true);
    }
    w.makeKeyAndOrderFront(None);
}

/// A frame's time on the screen the window is on.
fn frame_ms() -> f64 {
    let fps = window().and_then(|w| w.screen()).map_or(60, |s| s.maximumFramesPerSecond()).max(30);
    1000. / fps as f64
}

// ---------- the view ----------

#[derive(Default)]
struct ViewState {
    /// What an input method is composing (shown by the input method, not in the field).
    marked: RefCell<String>,
    /// A click held with Control: the Mac's right click, told as one when let go.
    control_click: Cell<bool>,
    /// The pointer's last place in the view (points), where an input method's window goes.
    pointer: Cell<NSPoint>,
}

define_class!(
    /// The window's one view, hosting the app's layers and taking its input.
    // SAFETY: NSView can be subclassed; nothing here is dropped specially.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZunflixView"]
    #[ivars = ViewState]
    struct View;

    impl View {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn backing_changed(&self) {
            if let Some(w) = self.window() {
                SCALE.set(w.backingScaleFactor());
            }
            fit_host();
            deliver(Event::Resized { minimised: false });
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let (x, y) = self.at(event);
            deliver(Event::PointerMoved { x, y });
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let (x, y) = self.at(event);
            deliver(Event::PointerMoved { x, y });
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            let (x, y) = self.at(event);
            deliver(Event::PointerMoved { x, y });
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            deliver(Event::PointerLeft);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let (x, y) = self.at(event);
            if event.modifierFlags().contains(NSEventModifierFlags::Control) {
                self.ivars().control_click.set(true);
                return;
            }
            let s = scale() as f32;
            // (the top strip, wherever nothing's clickable: the window moved, or zoomed with a double click)
            if CALLBACKS.get().is_some_and(|c| (c.drags)(x / s, y / s)) {
                if let Some(w) = self.window() {
                    if event.clickCount() == 2 {
                        title_double_click(&w);
                    } else {
                        w.performWindowDragWithEvent(event);
                    }
                }
                return;
            }
            deliver(Event::PointerDown { x, y });
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let (x, y) = self.at(event);
            if self.ivars().control_click.replace(false) {
                deliver(Event::RightUp { x, y });
            } else {
                deliver(Event::PointerUp { x, y });
            }
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            let (x, y) = self.at(event);
            deliver(Event::RightUp { x, y });
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let (x, y) = self.at(event);
            // (a trackpad's points are the app's units already; a wheel's lines are Windows' notches, 120 each)
            let k = if event.hasPreciseScrollingDeltas() { 1. } else { 120. };
            let (dx, dy) = (event.scrollingDeltaX() * k, event.scrollingDeltaY() * k);
            let ctrl = event.modifierFlags().contains(NSEventModifierFlags::Command);
            let (delta, horizontal) = if dx.abs() > dy.abs() { (dx, true) } else { (dy, false) };
            if delta != 0. {
                deliver(Event::Wheel { x, y, delta: delta as f32, horizontal, ctrl });
            }
        }

        // The Edit menu: each item is its Cmd shortcut, pressed or clicked.
        #[unsafe(method(copy:))]
        fn copy(&self, _sender: Option<&AnyObject>) {
            edit(b'C');
        }

        #[unsafe(method(cut:))]
        fn cut(&self, _sender: Option<&AnyObject>) {
            edit(b'X');
        }

        #[unsafe(method(paste:))]
        fn paste(&self, _sender: Option<&AnyObject>) {
            edit(b'V');
        }

        #[unsafe(method(selectAll:))]
        fn select_all(&self, _sender: Option<&AnyObject>) {
            edit(b'A');
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let composing = !self.ivars().marked.borrow().is_empty();
            if !composing {
                if let Some(vk) = key_of(event) {
                    deliver(Event::Key(vk));
                }
                if event.modifierFlags().contains(NSEventModifierFlags::Command) {
                    return;
                }
            }
            // (typed text, dead keys and input methods, through NSTextInputClient)
            let events = NSArray::from_slice(&[event]);
            self.interpretKeyEvents(&events);
        }
    }

    unsafe impl NSObjectProtocol for View {}

    unsafe impl NSTextInputClient for View {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, text: &AnyObject, _range: NSRange) {
            self.ivars().marked.borrow_mut().clear();
            let text = plain(text);
            for unit in text.encode_utf16() {
                deliver(Event::Char(unit));
            }
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command(&self, _selector: Sel) {
            // (the keys behind these commands, Return or the arrows, went to the app as keys already)
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text(&self, text: &AnyObject, _selected: NSRange, _range: NSRange) {
            *self.ivars().marked.borrow_mut() = plain(text);
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            self.ivars().marked.borrow_mut().clear();
            if let Some(input) = self.inputContext() {
                input.discardMarkedText();
            }
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            NSRange::new(NSNotFound as NSUInteger, 0)
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            let length = self.ivars().marked.borrow().encode_utf16().count();
            if length == 0 {
                NSRange::new(NSNotFound as NSUInteger, 0)
            } else {
                NSRange::new(0, length)
            }
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            !self.ivars().marked.borrow().is_empty()
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        fn attributed_substring(&self, _range: NSRange, _actual: NSRangePointer) -> Option<Retained<NSAttributedString>> {
            None
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes(&self) -> Retained<NSArray<NSString>> {
            NSArray::new()
        }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        fn first_rect(&self, _range: NSRange, _actual: NSRangePointer) -> NSRect {
            // (the input method's window by the pointer: the app's fields don't say where their caret is)
            let at = self.ivars().pointer.get();
            let local = NSRect::new(at, NSSize::new(1., 20.));
            match self.window() {
                Some(w) => w.convertRectToScreen(self.convertRect_toView(local, None)),
                None => local,
            }
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index(&self, _point: NSPoint) -> NSUInteger {
            NSNotFound as NSUInteger
        }
    }
);

impl View {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<View> {
        let this = View::alloc(mtm).set_ivars(ViewState::default());
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Where an event happened, in pixels from the view's top left.
    fn at(&self, event: &NSEvent) -> (f32, f32) {
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        self.ivars().pointer.set(p);
        let s = scale();
        ((p.x * s) as f32, (p.y * s) as f32)
    }
}

/// Typed text, whether it comes plain or with attributes.
fn plain(text: &AnyObject) -> String {
    if let Some(s) = text.downcast_ref::<NSString>() {
        s.to_string()
    } else if let Some(s) = text.downcast_ref::<NSAttributedString>() {
        s.string().to_string()
    } else {
        String::new()
    }
}

/// What a double click on the title bar does, as the Mac's settings have it (zoom unless set otherwise).
fn title_double_click(w: &NSWindow) {
    let action = NSUserDefaults::standardUserDefaults().stringForKey(&NSString::from_str("AppleActionOnDoubleClick"));
    match action.map(|a| a.to_string()).as_deref() {
        Some("Minimize") => w.performMiniaturize(None),
        Some("None") => {}
        _ => w.performZoom(None),
    }
}

/// A key as the seam's (Windows') key code: letters and digits by what they type, the rest by where they are.
fn key_of(event: &NSEvent) -> Option<u16> {
    let code = event.keyCode();
    let named = match code {
        0x24 | 0x4C => Some(key::RETURN),
        0x30 => Some(key::TAB),
        0x31 => Some(key::SPACE),
        0x33 => Some(key::BACK),
        0x35 => Some(key::ESCAPE),
        0x75 => Some(key::DELETE),
        0x73 => Some(key::HOME),
        0x77 => Some(key::END),
        0x74 => Some(key::PAGE_UP),
        0x79 => Some(key::PAGE_DOWN),
        0x7B => Some(key::LEFT),
        0x7C => Some(key::RIGHT),
        0x7D => Some(key::DOWN),
        0x7E => Some(key::UP),
        0x67 => Some(key::F11),
        0x45 => Some(key::PAD_PLUS),
        0x4E => Some(key::PAD_MINUS),
        0x18 => Some(key::PLUS),
        0x1B => Some(key::MINUS),
        0x52..=0x59 => Some(key::PAD_ZERO + (code - 0x52)),
        0x5B => Some(key::PAD_ZERO + 8),
        0x5C => Some(key::PAD_ZERO + 9),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let typed = event.charactersIgnoringModifiers()?.to_string();
    let c = typed.chars().next()?.to_ascii_uppercase();
    (c.is_ascii_uppercase() || c.is_ascii_digit()).then_some(c as u16)
}

// ---------- the window's delegate ----------

define_class!(
    /// The window's and the app's delegate: closing, resizing, minimising, quitting.
    // SAFETY: NSObject has no subclassing requirements; nothing here is dropped specially.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZunflixDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSWindow) -> bool {
            // (the app has its last word: handled, it stays open)
            !deliver(Event::Close)
        }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            let app = NSApplication::sharedApplication(self.mtm());
            app.stop(None);
            // (an event to wake the loop, so it sees it's been stopped)
            if let Some(nudge) = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
                NSEventType::ApplicationDefined,
                NSPoint::new(0., 0.),
                NSEventModifierFlags::empty(),
                0.,
                0,
                None,
                0,
                0,
                0,
            ) {
                app.postEvent_atStart(&nudge, false);
            }
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            fit_host();
            deliver(Event::Resized { minimised: false });
        }

        #[unsafe(method(windowDidMiniaturize:))]
        fn window_did_miniaturize(&self, _notification: &NSNotification) {
            deliver(Event::Resized { minimised: true });
        }

        #[unsafe(method(windowDidDeminiaturize:))]
        fn window_did_deminiaturize(&self, _notification: &NSNotification) {
            deliver(Event::Resized { minimised: false });
        }
    }

    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            // (in front once launched: asked before, a binary started from a shell stays behind the one that started it)
            #[allow(deprecated)]
            NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _sender: &NSApplication) -> NSApplicationTerminateReply {
            // (quitting closes the window, so the app has its last word as it does when it's closed)
            match window().filter(|w| w.isVisible()) {
                Some(w) => {
                    w.performClose(None);
                    NSApplicationTerminateReply::TerminateCancel
                }
                None => NSApplicationTerminateReply::TerminateNow,
            }
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Delegate> {
        let this = Delegate::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// An Edit menu item: its letter with Cmd held, as the app takes a shortcut.
fn edit(letter: u8) {
    MENU_CMD.set(true);
    deliver(Event::Key(u16::from(letter)));
    MENU_CMD.set(false);
}

/// The menu bar a Mac app has: the app's menu (hide, quit), Edit (cut, copy, paste, select all), View (full screen)
/// and the Window menu (minimise, zoom, close).
fn menu_bar(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let item = |title: &str, action: Sel, key: &str, mask: NSEventModifierFlags| {
        let i = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(title), Some(action), &NSString::from_str(key)) };
        i.setKeyEquivalentModifierMask(mask);
        i
    };
    let cmd = NSEventModifierFlags::Command;
    let submenu = |title: &str, items: Vec<Retained<NSMenuItem>>| {
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
        for i in &items {
            menu.addItem(i);
        }
        let holder = NSMenuItem::new(mtm);
        holder.setSubmenu(Some(&menu));
        (holder, menu)
    };
    let bar = NSMenu::new(mtm);
    let (app, _) = submenu("zunflix", vec![item("Hide zunflix", sel!(hide:), "h", cmd), item("Quit zunflix", sel!(terminate:), "q", cmd)]);
    let (edit, _) = submenu(
        "Edit",
        vec![
            item("Cut", sel!(cut:), "x", cmd),
            item("Copy", sel!(copy:), "c", cmd),
            item("Paste", sel!(paste:), "v", cmd),
            item("Select All", sel!(selectAll:), "a", cmd),
        ],
    );
    let (view, _) = submenu("View", vec![item("Enter Full Screen", sel!(toggleFullScreen:), "f", cmd | NSEventModifierFlags::Control)]);
    let (windows, window_menu) = submenu(
        "Window",
        vec![
            item("Minimize", sel!(performMiniaturize:), "m", cmd),
            item("Zoom", sel!(performZoom:), "", NSEventModifierFlags::empty()),
            item("Close", sel!(performClose:), "w", cmd),
        ],
    );
    bar.addItem(&app);
    bar.addItem(&edit);
    bar.addItem(&view);
    bar.addItem(&windows);
    NSApplication::sharedApplication(mtm).setWindowsMenu(Some(&window_menu));
    bar
}

// ---------- the seam ----------

/// The window (it's the one window on the UI thread).
#[derive(Clone, Copy)]
pub struct Window;

/// Wakes the window's thread (`Event::Wake`) from any thread.
#[derive(Clone, Copy)]
pub struct Waker;

/// Full screen's way back: the Mac's own full screen remembers where the window was.
pub struct Restore;

impl Wake for Waker {
    fn wake(&self) -> bool {
        unsafe { dispatch_async_f(&_dispatch_main_q, std::ptr::null_mut(), woken) };
        true
    }
}

impl Window {
    /// The compositor's root layer put in the view.
    pub(super) fn host(&self, layer: &CALayer) {
        VIEW.with(|v| {
            if let Some(host) = v.borrow().as_ref().and_then(|v| v.layer()) {
                host.addSublayer(layer);
            }
        });
    }

    fn is_full_screen(w: &NSWindow) -> bool {
        w.styleMask().contains(NSWindowStyleMask::FullScreen)
    }
}

impl Shell for Window {
    const OWN_BUTTONS: bool = false;

    fn open(opening: Opening, callbacks: Callbacks) -> Result<Option<Window>> {
        let Opening { setup, small, .. } = opening;
        let _ = SMALL.set(small);
        let mtm = MainThreadMarker::new().ok_or_else(|| crate::platform::Error::new("the window must open on the main thread"))?;
        let app = NSApplication::sharedApplication(mtm);
        // (one zunflix at a time: a second brings the first to the front; the uninstaller runs beside the app it
        // waits for)
        if !setup {
            if let Some(id) = NSBundle::mainBundle().bundleIdentifier() {
                let me = NSRunningApplication::currentApplication().processIdentifier();
                let others = NSRunningApplication::runningApplicationsWithBundleIdentifier(&id);
                if let Some(other) = others.iter().find(|a| a.processIdentifier() != me) {
                    #[allow(deprecated)]
                    other.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
                    return Ok(None);
                }
            }
        }
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.setMainMenu(Some(&menu_bar(mtm)));
        let delegate = Delegate::new(mtm);
        app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        CALLBACKS.set(Some(callbacks));

        let (w, h) = if setup { (small.0 as f64, small.1 as f64) } else { (1180., 780.) };
        let style = if setup {
            // (the bootstrapper's card is drawn round by the app, on a window with no frame of its own)
            NSWindowStyleMask::Borderless | NSWindowStyleMask::FullSizeContentView
        } else {
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::FullSizeContentView
        };
        let frame = NSRect::new(NSPoint::new(0., 0.), NSSize::new(w, h));
        let window = unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), frame, style, NSBackingStoreType::Buffered, false) };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("zunflix"));
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setAppearance(NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }).as_deref());
        window.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenPrimary);
        if setup {
            window.setOpaque(false);
            window.setBackgroundColor(Some(&NSColor::clearColor()));
            window.setHasShadow(true);
            window.setAnimationBehavior(NSWindowAnimationBehavior::None);
        } else {
            let c = crate::tokens::C_BG;
            let part = |shift: u32| (c >> shift & 255) as f64 / 255.;
            window.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(part(16), part(8), part(0), 1.)));
        }
        window.center();
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        let view = View::new(mtm, frame);
        let host = super::compositor::host_layer();
        view.setLayer(Some(&host));
        view.setWantsLayer(true);
        view.setLayerUsesCoreImageFilters(true);
        let options = NSTrackingAreaOptions::MouseEnteredAndExited | NSTrackingAreaOptions::MouseMoved | NSTrackingAreaOptions::ActiveAlways | NSTrackingAreaOptions::InVisibleRect;
        let tracking = unsafe { NSTrackingArea::initWithRect_options_owner_userInfo(NSTrackingArea::alloc(), NSRect::ZERO, options, Some(&view), None) };
        view.addTrackingArea(&tracking);
        window.setContentView(Some(&view));
        window.makeFirstResponder(Some(&view));
        SCALE.set(window.backingScaleFactor());

        WINDOW.with(|w| *w.borrow_mut() = Some(window));
        VIEW.with(|v| *v.borrow_mut() = Some(view));
        DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
        fit_host();
        Ok(Some(Window))
    }

    fn run(&self) {
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).run();
        }
    }

    fn scale(&self) -> f32 {
        scale() as f32
    }

    fn client_size(&self) -> (i32, i32) {
        let size = VIEW.with(|v| v.borrow().as_ref().map(|v| v.bounds().size)).unwrap_or(NSSize::new(0., 0.));
        let s = scale();
        ((size.width * s).round() as i32, (size.height * s).round() as i32)
    }

    fn is_maximised(&self) -> bool {
        window().is_some_and(|w| w.isZoomed())
    }

    fn set_icon(&self, ico: &Path) {
        // (a bundle has its own icon; run on its own, the binary shows the app's)
        if NSBundle::mainBundle().bundleIdentifier().is_some() {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else { return };
        if let Some(image) = NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(&ico.to_string_lossy())) {
            unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
        }
    }

    fn show(&self) {
        if let Some(w) = window() {
            w.makeKeyAndOrderFront(None);
        }
    }

    fn to_front(&self) {
        if let Some(mtm) = MainThreadMarker::new() {
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        }
    }

    fn hold_hidden(&self) {
        HIDDEN_START.store(true, Ordering::Relaxed);
    }

    fn is_held_hidden(&self) -> bool {
        HIDDEN_START.load(Ordering::Relaxed)
    }

    fn show_held(&self) -> bool {
        if !HIDDEN_START.swap(false, Ordering::Relaxed) {
            return false;
        }
        if let Some(w) = window() {
            if let Some(screen) = w.screen() {
                w.setFrame_display(screen.visibleFrame(), true);
            }
            w.makeKeyAndOrderFront(None);
        }
        true
    }

    fn hide(&self) {
        if let Some(w) = window() {
            w.orderOut(None);
        }
    }

    fn close(&self) {
        after(0., 0, close_now);
    }

    fn command(&self, command: WindowCommand) {
        let Some(w) = window() else { return };
        match command {
            WindowCommand::Minimise => w.miniaturize(None),
            WindowCommand::Maximise if !w.isZoomed() => w.zoom(None),
            WindowCommand::Restore if w.isZoomed() => w.zoom(None),
            WindowCommand::Maximise | WindowCommand::Restore => {}
            WindowCommand::Close => after(0., 0, close_now),
        }
    }

    fn bootstrap(&self) {
        after(0., 0, into_bootstrap);
    }

    fn set_transitions(&self, on: bool) {
        if let Some(w) = window() {
            w.setAnimationBehavior(if on { NSWindowAnimationBehavior::Default } else { NSWindowAnimationBehavior::None });
        }
    }

    fn enter_full_screen(&self) -> Option<Restore> {
        let w = window()?;
        if !Window::is_full_screen(&w) {
            w.toggleFullScreen(None);
        }
        Some(Restore)
    }

    fn leave_full_screen(&self, _restore: &Restore) {
        if let Some(w) = window().filter(|w| Window::is_full_screen(w)) {
            w.toggleFullScreen(None);
        }
    }

    fn wait_for_refresh(&self) {
        // (Core Animation commits each frame in step with the display itself: nothing to wait for)
    }

    fn start_frame_timer(&self) -> bool {
        let (_, start) = FRAME.get();
        let start = start + 1;
        FRAME.set((true, start));
        after(frame_ms(), start as usize, frame_due);
        true
    }

    fn stop_frame_timer(&self) {
        let (_, start) = FRAME.get();
        FRAME.set((false, start));
    }

    fn start_ticks(&self) {
        after(200., 0, tick);
    }

    fn waker(&self) -> Waker {
        Waker
    }

    fn clipboard(&self) -> Option<Vec<u16>> {
        let text = NSPasteboard::generalPasteboard().stringForType(unsafe { NSPasteboardTypeString })?;
        Some(text.to_string().encode_utf16().collect())
    }

    fn set_clipboard(&self, text: &[u16]) {
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        board.setString_forType(&NSString::from_str(&String::from_utf16_lossy(text)), unsafe { NSPasteboardTypeString });
    }

    fn pick_file(&self, _kind: &str, patterns: &str) -> Option<String> {
        let mtm = MainThreadMarker::new()?;
        let panel = NSOpenPanel::openPanel(mtm);
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(false);
        let types: Vec<Retained<NSString>> = patterns.split(';').filter_map(|p| p.trim().strip_prefix("*.")).map(NSString::from_str).collect();
        #[allow(deprecated)]
        panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&types)));
        // (NSModalResponseOK)
        if panel.runModal() != 1 {
            return None;
        }
        Some(panel.URL()?.path()?.to_string())
    }
}
