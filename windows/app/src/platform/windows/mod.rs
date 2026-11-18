//! Windows under the seam: the compositor (Windows.UI.Composition) drawn on with Direct2D and DirectWrite, Win32 for
//! the window, WIC for pictures, Media Foundation for sound, the setup exe for installing.
mod compositor;
pub mod install;
mod picture;
pub mod video;
mod window;

pub use compositor::{Brush, Canvas, ColorBrush, Graphics, Surface, Visual};
pub use picture::Picture;
pub use window::{Restore, Waker, Window};

use crate::platform::{key, Cursor, Error, Loader, Result};
use std::ffi::c_void;
use std::path::Path;
use windows::core::{HRESULT, HSTRING, PCSTR};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// D2DERR_RECREATE_TARGET: Direct2D's own word for a device gone.
const RECREATE_TARGET: HRESULT = HRESULT(0x8899000C_u32 as i32);

impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Error {
        if [DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET, RECREATE_TARGET].contains(&e.code()) {
            Error::lost_device(e.to_string())
        } else {
            Error::new(e.to_string())
        }
    }
}

// The seam's key numbers are Windows' own virtual-key codes: checked here, so they can't drift.
const _: () = {
    let pairs = [
        (key::BACK, VK_BACK), (key::TAB, VK_TAB), (key::RETURN, VK_RETURN), (key::ESCAPE, VK_ESCAPE), (key::SPACE, VK_SPACE),
        (key::PAGE_UP, VK_PRIOR), (key::PAGE_DOWN, VK_NEXT), (key::END, VK_END), (key::HOME, VK_HOME), (key::LEFT, VK_LEFT),
        (key::UP, VK_UP), (key::RIGHT, VK_RIGHT), (key::DOWN, VK_DOWN), (key::DELETE, VK_DELETE), (key::ZERO, VK_0),
        (key::A, VK_A), (key::C, VK_C), (key::F, VK_F), (key::J, VK_J), (key::K, VK_K), (key::L, VK_L), (key::M, VK_M),
        (key::N, VK_N), (key::V, VK_V), (key::X, VK_X), (key::PAD_ZERO, VK_NUMPAD0), (key::PAD_PLUS, VK_ADD),
        (key::PAD_MINUS, VK_SUBTRACT), (key::F11, VK_F11), (key::PLUS, VK_OEM_PLUS), (key::MINUS, VK_OEM_MINUS),
    ];
    let mut i = 0;
    while i < pairs.len() {
        assert!(pairs[i].0 == pairs[i].1 .0);
        i += 1;
    }
};

/// What the server is told this device is: `net::device_label` puts it on the account page.
pub const MODEL: &str = "App on Windows";

/// The Windows release and build ("Windows 11 26200"), read from the registry: `GetVersionEx` reports 8 on apps
/// without a manifest for it. Build 22000 is where Windows 11 starts.
pub fn os_name() -> &'static str {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    static OS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    OS.get_or_init(|| {
        let mut buf = [0u16; 16];
        let mut bytes = std::mem::size_of_val(&buf) as u32;
        let read = unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"), w!("CurrentBuildNumber"), RRF_RT_REG_SZ,
                None, Some(buf.as_mut_ptr().cast()), Some(&mut bytes))
        };
        let build = String::from_utf16_lossy(&buf[..(bytes as usize / 2).saturating_sub(1).min(buf.len())]);
        match (read.is_ok(), build.parse::<u32>()) {
            (true, Ok(n)) => format!("Windows {} {n}", if n >= 22000 { 11 } else { 10 }),
            _ => "Windows".into(),
        }
    })
}

/// Shows an error the app can't carry on after.
pub fn alert(text: &str) {
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), windows::core::w!("zunflix"), MB_ICONERROR);
    }
}

pub fn set_cursor(cursor: Cursor) {
    let id = match cursor {
        Cursor::Arrow => IDC_ARROW,
        Cursor::Hand => IDC_HAND,
        Cursor::Move => IDC_SIZEALL,
        Cursor::None => {
            unsafe { SetCursor(None) };
            return;
        }
    };
    unsafe {
        if let Ok(cursor) = LoadCursorW(None, id) {
            SetCursor(Some(cursor));
        }
    }
}

/// Whether Ctrl is held as the current message came.
pub fn ctrl_down() -> bool {
    unsafe { GetKeyState(VK_CONTROL.0 as i32) < 0 }
}

/// Whether Shift is held as the current message came.
pub fn shift_down() -> bool {
    unsafe { GetKeyState(VK_SHIFT.0 as i32) < 0 }
}

/// This computer's offset from UTC now, in seconds (daylight saving taken into account).
pub fn utc_offset_seconds() -> i32 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    let mut zone = TIME_ZONE_INFORMATION::default();
    // (1: standard time now, 2: daylight time; anything else, no such split)
    let bias = match unsafe { GetTimeZoneInformation(&mut zone) } {
        1 => zone.Bias + zone.StandardBias,
        2 => zone.Bias + zone.DaylightBias,
        _ => zone.Bias,
    };
    -bias * 60
}

/// The bytes free on the drive `dir` is on.
pub fn free_bytes(dir: &Path) -> Option<u64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut free = 0;
    unsafe { GetDiskFreeSpaceExW(&HSTRING::from(dir.as_os_str()), Some(&mut free), None, None) }.ok().map(|()| free)
}

/// A DLL, freed when this goes.
pub struct Library(HMODULE);

impl Loader for Library {
    fn load(name: &str) -> Result<Library> {
        Ok(Library(unsafe { LoadLibraryW(&HSTRING::from(name))? }))
    }

    fn symbol(&self, name: &str) -> Result<*const c_void> {
        let name = std::ffi::CString::new(name).map_err(|e| Error::new(e.to_string()))?;
        unsafe { GetProcAddress(self.0, PCSTR(name.as_ptr().cast())) }
            .map(|f| f as *const c_void)
            .ok_or_else(|| windows::core::Error::from_thread().into())
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}
