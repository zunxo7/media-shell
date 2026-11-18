//! The player loads libmpv on demand, on its own thread.
use crate::platform::{video, Error, Library, Loader, Result};
use std::ffi::{c_char, c_int, c_void, CStr, CString};

pub type Handle = *mut c_void;

#[repr(C)]
pub struct Event {
    pub id: c_int,
    pub error: c_int,
    pub reply: u64,
    pub data: *mut c_void,
}
#[repr(C)]
pub struct EndFile {
    pub reason: c_int,
    pub error: c_int,
}

pub const EV_SHUTDOWN: c_int = 1;
pub const EV_END_FILE: c_int = 7;
pub const EV_FILE_LOADED: c_int = 8;
pub const EV_VIDEO_RECONFIG: c_int = 17;
pub const EV_PLAYBACK_RESTART: c_int = 21;

/// One `mpv_render_param`: what it is and a pointer to its value; a list of them ends with a zero `kind`.
#[cfg(target_os = "macos")]
#[repr(C)]
pub struct RenderParam {
    pub kind: c_int,
    pub data: *mut c_void,
}
#[cfg(target_os = "macos")]
pub type RenderContext = *mut c_void;

macro_rules! api {
    ($($(#[$m:meta])* $name:ident : fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        pub struct Api { _lib: Library, $($(#[$m])* pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)* }
        impl Api {
            pub fn load() -> Result<Api> {
                let lib = Library::load(video::LIBRARY)?;
                Ok(Api {
                    $($(#[$m])* $name: unsafe { std::mem::transmute::<*const c_void, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(lib.symbol(stringify!($name))?) },)*
                    _lib: lib,
                })
            }
        }
    };
}
api! {
    mpv_create: fn() -> Handle;
    mpv_initialize: fn(Handle) -> c_int;
    mpv_set_option_string: fn(Handle, *const c_char, *const c_char) -> c_int;
    mpv_set_property_string: fn(Handle, *const c_char, *const c_char) -> c_int;
    mpv_get_property_string: fn(Handle, *const c_char) -> *mut c_char;
    mpv_command: fn(Handle, *const *const c_char) -> c_int;
    mpv_wait_event: fn(Handle, f64) -> *mut Event;
    mpv_wakeup: fn(Handle);
    mpv_free: fn(*mut c_void);
    mpv_terminate_destroy: fn(Handle);
    mpv_error_string: fn(c_int) -> *const c_char;
    // The render API, for the Mac, where the app draws mpv's picture; Windows' mpv presents its own swap chain.
    #[cfg(target_os = "macos")]
    mpv_render_context_create: fn(*mut RenderContext, Handle, *mut RenderParam) -> c_int;
    #[cfg(target_os = "macos")]
    mpv_render_context_set_update_callback: fn(RenderContext, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void);
    #[cfg(target_os = "macos")]
    mpv_render_context_update: fn(RenderContext) -> u64;
    #[cfg(target_os = "macos")]
    mpv_render_context_render: fn(RenderContext, *mut RenderParam) -> c_int;
    #[cfg(target_os = "macos")]
    mpv_render_context_free: fn(RenderContext);
}

pub struct Mpv {
    pub api: Api,
    pub h: Handle,
}
impl Drop for Mpv {
    fn drop(&mut self) { unsafe { (self.api.mpv_terminate_destroy)(self.h); } }
}

impl Mpv {
    pub fn new() -> Result<Mpv> {
        let api = Api::load()?;
        let h = unsafe { (api.mpv_create)() };
        if h.is_null() { return Err(Error::new("mpv_create failed")); }
        Ok(Mpv { api, h })
    }
    pub fn opt(&self, k: &str, v: &str) -> bool {
        let (a, b) = (CString::new(k).unwrap(), CString::new(v).unwrap());
        unsafe { (self.api.mpv_set_option_string)(self.h, a.as_ptr(), b.as_ptr()) >= 0 }
    }
    pub fn set(&self, k: &str, v: &str) -> c_int {
        let Ok(b) = CString::new(v) else { return -4 };
        let a = CString::new(k).unwrap();
        unsafe { (self.api.mpv_set_property_string)(self.h, a.as_ptr(), b.as_ptr()) }
    }
    pub fn get(&self, k: &str) -> Option<String> {
        let a = CString::new(k).unwrap();
        unsafe {
            let p = (self.api.mpv_get_property_string)(self.h, a.as_ptr());
            if p.is_null() {
                return None;
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            (self.api.mpv_free)(p as *mut c_void);
            Some(s)
        }
    }
    pub fn cmd(&self, args: &[&str]) -> c_int {
        let Ok(owned): std::result::Result<Vec<CString>, _> = args.iter().map(|a| CString::new(*a)).collect() else { return -4 };
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        unsafe { (self.api.mpv_command)(self.h, ptrs.as_ptr()) }
    }
    pub fn err(&self, code: c_int) -> String {
        unsafe { CStr::from_ptr((self.api.mpv_error_string)(code)).to_string_lossy().into_owned() }
    }
}
