//! libmpv's picture on Windows: its d3d11 output in composition mode draws into a swap chain, which the compositor
//! shows under the app's visuals through a brush.
use crate::mpv::Mpv;
use crate::platform::Result;
use windows::core::{IUnknown, Interface};

/// libmpv's file, beside the app.
pub const LIBRARY: &str = "libmpv-2.dll";

/// The options that make mpv draw into a surface the app shows, set before the rest.
pub const OPTIONS: &[(&str, &str)] = &[("vo", "gpu-next"), ("gpu-api", "d3d11"), ("gpu-context", "d3d11"), ("d3d11-output-mode", "composition")];

/// The property its picture's size in pixels goes in, as "WxH".
const SIZE_PROPERTY: &str = "d3d11-composition-size";

/// The property mpv names its swap chain by (an address), looked for as it's made or remade.
const SURFACE_PROPERTY: &str = "display-swapchain";

/// The picture's producer, on the player thread: mpv's own swap chain, sized here.
pub struct Output {
    /// The swap chain last handed to the compositor (its address).
    attached: usize,
}

impl Output {
    /// After mpv_initialize, before the first loadfile.
    pub fn new(mpv: &Mpv, w: i32, h: i32) -> Result<Output> {
        let mut output = Output { attached: 0 };
        output.resize(mpv, w, h);
        Ok(output)
    }

    pub fn resize(&mut self, mpv: &Mpv, w: i32, h: i32) {
        mpv.set(SIZE_PROPERTY, &format!("{w}x{h}"));
    }

    /// A new swap chain for the compositor to show, if mpv has made one since the last (looked for when `due`).
    pub fn surface(&mut self, mpv: &Mpv, due: bool) -> Option<Surface> {
        let raw = due
            .then(|| mpv.get(SURFACE_PROPERTY))
            .flatten()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|p| *p != 0 && *p != self.attached)?;
        let surface = Surface::adopt(raw)?;
        self.attached = raw;
        Some(surface)
    }
}

/// mpv's surface, held (a reference of its own) until a brush shows it; sent from mpv's thread to the UI's.
pub struct Surface(usize);

impl Surface {
    /// The surface at the address mpv gave, held; None for none.
    fn adopt(address: usize) -> Option<Surface> {
        let borrowed = address as *mut std::ffi::c_void;
        let swap = unsafe { IUnknown::from_raw_borrowed(&borrowed) }?;
        Some(Surface(swap.clone().into_raw() as usize))
    }

    pub(super) fn into_swap_chain(self) -> IUnknown {
        let swap = unsafe { IUnknown::from_raw(self.0 as _) };
        std::mem::forget(self);
        swap
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        drop(unsafe { IUnknown::from_raw(self.0 as _) });
    }
}
