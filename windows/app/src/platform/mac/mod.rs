//! macOS under the seam: Core Animation layers drawn on with Core Graphics and Core Text, AppKit for the window,
//! ImageIO for pictures.
mod compositor;
mod draw;
pub mod install;
mod picture;
mod text;
pub mod video;
mod window;

pub use compositor::{Brush, ColorBrush, Graphics, Surface, Visual};
pub use draw::Canvas;
pub use picture::Picture;
pub use window::{Restore, Waker, Window};

use crate::platform::{Cursor, Error, Loader, Result};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertStyle, NSCursor, NSEvent, NSEventModifierFlags};
use objc2_foundation::{NSFileManager, NSFileSystemFreeSize, NSNumber, NSProcessInfo, NSString, NSTimeZone};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;

/// What the server is told this device is: `net::device_label` puts it on the account page.
pub const MODEL: &str = "App on Mac";

/// The macOS release ("macOS 15.1").
pub fn os_name() -> &'static str {
    static OS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    OS.get_or_init(|| {
        let v = NSProcessInfo::processInfo().operatingSystemVersion();
        format!("macOS {}.{}", v.majorVersion, v.minorVersion)
    })
}

/// Shows an error the app can't carry on after.
pub fn alert(text: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("zunflix: {text}");
        return;
    };
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("zunflix"));
    alert.setInformativeText(&NSString::from_str(text));
    alert.setAlertStyle(NSAlertStyle::Critical);
    alert.runModal();
}

pub fn set_cursor(cursor: Cursor) {
    let shown = match cursor {
        Cursor::Arrow => NSCursor::arrowCursor(),
        Cursor::Hand => NSCursor::pointingHandCursor(),
        // (the Mac's hands: open over something that can be moved, closed while it's held)
        Cursor::Move if NSEvent::pressedMouseButtons() & 1 != 0 => NSCursor::closedHandCursor(),
        Cursor::Move => NSCursor::openHandCursor(),
        Cursor::None => {
            NSCursor::setHiddenUntilMouseMoves(true);
            return;
        }
    };
    shown.set();
}

/// Whether Cmd (the Mac's Ctrl for shortcuts) is held as the current event came.
pub fn ctrl_down() -> bool {
    NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Command) || window::MENU_CMD.get()
}

/// Whether Shift is held as the current event came.
pub fn shift_down() -> bool {
    NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Shift)
}

/// This computer's offset from UTC now, in seconds (daylight saving taken into account).
pub fn utc_offset_seconds() -> i32 {
    NSTimeZone::localTimeZone().secondsFromGMT() as i32
}

/// The bytes free on the volume `dir` is on.
pub fn free_bytes(dir: &Path) -> Option<u64> {
    let path = NSString::from_str(&dir.to_string_lossy());
    let attributes = NSFileManager::defaultManager().attributesOfFileSystemForPath_error(&path).ok()?;
    let free = attributes.objectForKey(unsafe { NSFileSystemFreeSize })?;
    Some(free.downcast::<NSNumber>().ok()?.unsignedLongLongValue())
}

extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
    fn dlerror() -> *const c_char;
}
const RTLD_NOW: c_int = 2;
const RTLD_LOCAL: c_int = 4;

fn dl_error() -> Error {
    let message = unsafe { dlerror() };
    if message.is_null() {
        Error::new("the library couldn't be loaded")
    } else {
        Error::new(unsafe { CStr::from_ptr(message) }.to_string_lossy())
    }
}

/// A dynamic library, closed when this goes.
pub struct Library(*mut c_void);

impl Loader for Library {
    /// Looked for beside the app first (the bundle's Frameworks, then its own folder), as Windows looks beside the exe.
    fn load(name: &str) -> Result<Library> {
        let exe = std::env::current_exe().ok();
        let dir = exe.as_deref().and_then(Path::parent);
        let candidates = dir
            .map(|d| vec![d.join("../Frameworks").join(name), d.join(name)])
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.exists())
            .map(|p| p.to_string_lossy().into_owned())
            .chain(std::iter::once(name.to_string()));
        for path in candidates {
            let c = CString::new(path).map_err(|e| Error::new(e.to_string()))?;
            let handle = unsafe { dlopen(c.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
            if !handle.is_null() {
                return Ok(Library(handle));
            }
        }
        Err(dl_error())
    }

    fn symbol(&self, name: &str) -> Result<*const c_void> {
        let name = CString::new(name).map_err(|e| Error::new(e.to_string()))?;
        let f = unsafe { dlsym(self.0, name.as_ptr()) };
        if f.is_null() {
            Err(dl_error())
        } else {
            Ok(f as *const c_void)
        }
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            dlclose(self.0);
        }
    }
}
