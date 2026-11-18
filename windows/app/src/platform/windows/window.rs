//! The app's Win32 window: no system title bar (the window keeps its normal style, so snapping, the minimise and
//! maximise animations and double-clicking the top all still work), its messages turned into the seam's events.
use crate::platform::{Callbacks, Event, Opening, Result, Shell, Wake, WindowCommand};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, ScreenToClient, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::{LibraryLoader::*, Threading::*, WinRT::*};
use windows::Win32::UI::{HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*};

/// Work from other threads waiting (`Waker`).
const UPDATE: u32 = WM_APP + 1;
/// The window made the bootstrapper's: small, in the middle of the screen (an update about to be fetched).
const BOOTSTRAP: u32 = WM_APP + 2;
/// The pointer left the window (asked for with TrackMouseEvent).
const WM_MOUSELEAVE: u32 = 0x02A3;
const FRAME_TIMER: usize = 2;
const TICK_TIMER: usize = 1;

/// Opened without being shown: it shows once the update check has answered, either as the bootstrapper (an update)
/// or maximised, so an update never shows the app's window for a moment first.
static HIDDEN_START: AtomicBool = AtomicBool::new(false);
/// The setup's and the bootstrapper's size (layout units).
static SMALL: OnceLock<(f32, f32)> = OnceLock::new();

thread_local! {
    static CALLBACKS: Cell<Option<Callbacks>> = const { Cell::new(None) };
    /// The compositor's dispatcher queue on this thread, kept for the app's life.
    static QUEUE: RefCell<Option<windows::System::DispatcherQueueController>> = const { RefCell::new(None) };
}

/// The window.
#[derive(Clone, Copy)]
pub struct Window(HWND);

/// Wakes the window's thread (`Event::Wake`) from any thread.
#[derive(Clone, Copy)]
pub struct Waker(isize);

/// Where the window was before it filled the screen.
pub struct Restore(WINDOWPLACEMENT);

impl Window {
    pub(super) fn hwnd(&self) -> HWND {
        self.0
    }
}

impl Wake for Waker {
    fn wake(&self) -> bool {
        unsafe { PostMessageW(Some(HWND(self.0 as _)), UPDATE, WPARAM(0), LPARAM(0)) }.is_ok()
    }
}

/// The window as the bootstrapper: restored, small, in the middle of the work area.
fn into_bootstrap(hwnd: HWND) {
    HIDDEN_START.store(false, Ordering::Relaxed);
    let (bw, bh) = SMALL.get().copied().unwrap_or_default();
    unsafe {
        let k = GetDpiForWindow(hwnd) as f32 / 96.;
        let (w, h) = ((bw * k) as i32, (bh * k) as i32);
        let mut area = RECT::default();
        let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut area as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetWindowPos(hwnd, None, area.left + ((area.right - area.left) - w) / 2, area.top + ((area.bottom - area.top) - h) / 2, w, h, SWP_NOZORDER);
    }
}

fn set_attribute<T>(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, value: &T) {
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, attribute, (value as *const T).cast(), size_of::<T>() as u32);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    if msg == BOOTSTRAP {
        into_bootstrap(hwnd);
        return LRESULT(0);
    }
    let callbacks = CALLBACKS.get();
    // Maximised, Windows makes a window larger than the screen by its frame: that much is kept back so nothing goes off
    // the edges.
    let framed = (GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION.0) == WS_CAPTION.0;
    // switched back to (Alt+Tab, the taskbar): the keyboard focus taken back at once, so the wheel (which Windows sends to
    // the focus window) scrolls straight away, not only after a click
    if msg == WM_ACTIVATE && (wp.0 & 0xFFFF) as u32 != WA_INACTIVE {
        let _ = SetFocus(Some(hwnd));
    }
    if framed && msg == WM_NCCALCSIZE && wp.0 != 0 {
        // (as Windows Terminal does: the frame worked out as usual, keeping its sides and foot, where the edges
        // resize it, then the title bar taken back; maximised, the top kept back by the frame Windows puts off-screen)
        let params = &mut *(lp.0 as *mut NCCALCSIZE_PARAMS);
        let top = params.rgrc[0].top;
        let r = DefWindowProcW(hwnd, msg, wp, lp);
        let params = &mut *(lp.0 as *mut NCCALCSIZE_PARAMS);
        let mut inset = 0;
        if IsZoomed(hwnd).as_bool() {
            let dpi = GetDpiForWindow(hwnd);
            inset = GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
        }
        params.rgrc[0].top = top + inset;
        return r;
    }
    if framed && msg == WM_GETMINMAXINFO {
        // maximised: the screen's work area, the taskbar left showing (without a title bar Windows can otherwise make
        // it the whole screen); the frame it puts off the edges as usual, taken back in WM_NCCALCSIZE
        let info = &mut *(lp.0 as *mut MINMAXINFO);
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(monitor, &mut mi).as_bool() {
            let dpi = GetDpiForWindow(hwnd);
            let pad = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
            let fx = GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) + pad;
            let fy = GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) + pad;
            let (work, whole) = (mi.rcWork, mi.rcMonitor);
            info.ptMaxPosition = POINT { x: work.left - whole.left - fx, y: work.top - whole.top - fy };
            info.ptMaxSize = POINT { x: (work.right - work.left) + fx * 2, y: (work.bottom - work.top) + fy * 2 };
        }
        return LRESULT(0);
    }
    if framed && msg == WM_NCHITTEST {
        // the sides and foot: the frame's own (resizing); the top edge resizes too, a few pixels in
        let native = DefWindowProcW(hwnd, msg, wp, lp);
        if native.0 as u32 != HTCLIENT {
            return native;
        }
        let mut p = POINT { x: (lp.0 as i16) as i32, y: ((lp.0 >> 16) as i16) as i32 };
        let _ = ScreenToClient(hwnd, &mut p);
        let scale = GetDpiForWindow(hwnd) as f32 / 96.;
        let sizable = GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_THICKFRAME.0 != 0;
        if sizable && !IsZoomed(hwnd).as_bool() && p.y < (5. * scale) as i32 {
            return LRESULT(HTTOP as isize);
        }
        // the strip along the top drags it, wherever nothing clickable is
        if callbacks.is_some_and(|c| (c.drags)(p.x as f32 / scale, p.y as f32 / scale)) {
            return LRESULT(HTCAPTION as isize);
        }
        return LRESULT(HTCLIENT as isize);
    }
    let mut p = POINT { x: (lp.0 as i16) as i32, y: ((lp.0 >> 16) as i16) as i32 };
    if matches!(msg, WM_MOUSEWHEEL | WM_MOUSEHWHEEL) {
        let _ = ScreenToClient(hwnd, &mut p);
    }
    let (x, y) = (p.x as f32, p.y as f32);
    let event = match msg {
        UPDATE => Some(Event::Wake),
        WM_SIZE => Some(Event::Resized { minimised: wp.0 == SIZE_MINIMIZED as usize }),
        WM_TIMER if wp.0 == FRAME_TIMER => Some(Event::Frame),
        WM_TIMER => Some(Event::Tick),
        WM_MOUSEMOVE => {
            let mut track = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            let _ = TrackMouseEvent(&mut track);
            Some(Event::PointerMoved { x, y })
        }
        WM_MOUSELEAVE => Some(Event::PointerLeft),
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            Some(Event::PointerDown { x, y })
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            Some(Event::PointerUp { x, y })
        }
        WM_RBUTTONUP => Some(Event::RightUp { x, y }),
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => Some(Event::Wheel {
            x,
            y,
            delta: ((wp.0 >> 16) as i16) as f32,
            // (Shift turns the wheel sideways; MK_SHIFT and MK_CONTROL in the low word)
            horizontal: msg == WM_MOUSEHWHEEL || (wp.0 & 4 != 0),
            ctrl: wp.0 & 0x0008 != 0,
        }),
        WM_CHAR => Some(Event::Char(wp.0 as u16)),
        WM_KEYDOWN => Some(Event::Key(wp.0 as u16)),
        WM_CLOSE => Some(Event::Close),
        _ => None,
    };
    let handled = match (event, callbacks) {
        (Some(e), Some(c)) => (c.event)(e),
        _ => false,
    };
    if handled {
        LRESULT(0)
    } else {
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

impl Shell for Window {
    const OWN_BUTTONS: bool = true;

    fn open(opening: Opening, callbacks: Callbacks) -> Result<Option<Window>> {
        let Opening { setup: installing, uninstalling, small } = opening;
        let _ = SMALL.set(small);
        let (boot_w, boot_h) = small;
        unsafe {
            let instance = CreateMutexW(None, true, if uninstalling { w!("Local\\zunflix.desktop.uninstall") } else if installing { w!("Local\\zunflix.desktop.setup") } else { w!("Local\\zunflix.desktop.single-instance") })?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                if let Ok(window) = FindWindowW(w!("ZunflixDesktop"), None) {
                    let _ = ShowWindow(window, SW_RESTORE);
                    let _ = SetForegroundWindow(window);
                }
                let _ = CloseHandle(instance);
                return Ok(None);
            }
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            RoInitialize(RO_INIT_SINGLETHREADED)?;
            let module = GetModuleHandleW(None)?;
            let class = if installing { w!("ZunflixSetup") } else { w!("ZunflixDesktop") };
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: module.into(),
                lpszClassName: class,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                ..Default::default()
            });
            // (the setup opens in the middle of the screen)
            let (mut x, mut y) = (CW_USEDEFAULT, CW_USEDEFAULT);
            if installing {
                let k = GetDpiForSystem() as f32 / 96.;
                let mut area = RECT::default();
                let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut area as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
                x = area.left + ((area.right - area.left) - (boot_w * k) as i32) / 2;
                y = area.top + ((area.bottom - area.top) - (boot_h * k) as i32) / 2;
            }
            CALLBACKS.set(Some(callbacks));
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                class,
                w!("zunflix"),
                if installing { WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX } else { WS_OVERLAPPEDWINDOW },
                x,
                y,
                if installing { (boot_w * GetDpiForSystem() as f32 / 96.) as i32 } else { 1180 },
                if installing { (boot_h * GetDpiForSystem() as f32 / 96.) as i32 } else { 780 },
                None,
                None,
                Some(module.into()),
                None,
            )?;
            // (its frame worked out again without the title bar; a hairline of frame kept so Windows still gives it its
            // shadow and rounded corners: the app's window and the bootstrapper's alike)
            let _ = DwmExtendFrameIntoClientArea(hwnd, &windows::Win32::UI::Controls::MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 });
            let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            // (rounded corners, as Windows 11 gives windows; the bootstrapper's small window rounder still: Windows' own
            // rounding and frame off there, its corners see-through, its card drawn round (ui::bootstrap_view))
            if installing {
                let _ = DwmExtendFrameIntoClientArea(hwnd, &windows::Win32::UI::Controls::MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 0, cyBottomHeight: 0 });
                set_attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_DONOTROUND.0);
                // (DWMWA_COLOR_NONE: no border)
                set_attribute(hwnd, DWMWA_BORDER_COLOR, &0xFFFFFFFEu32);
                // (no frame drawn by Windows at all: no square shadow round the rounded card)
                set_attribute(hwnd, DWMWA_NCRENDERING_POLICY, &DWMNCRP_DISABLED.0);
            } else {
                set_attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_ROUND.0);
            }
            set_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &1i32);
            // (Windows' own animations for minimising, maximising and restoring: kept for the app, off only for the setup)
            set_attribute(hwnd, DWMWA_TRANSITIONS_FORCEDISABLED, &BOOL(i32::from(installing)));
            let queue = CreateDispatcherQueueController(DispatcherQueueOptions {
                dwSize: size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            })?;
            QUEUE.with(|q| *q.borrow_mut() = Some(queue));
            set_attribute(hwnd, DWMWA_CAPTION_COLOR, &0x00050505u32);
            set_attribute(hwnd, DWMWA_TEXT_COLOR, &0x00b8b8b8u32);
            Ok(Some(Window(hwnd)))
        }
    }

    fn run(&self) {
        let idle = CALLBACKS.get().map(|c| c.idle);
        unsafe {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                if let Some(idle) = idle {
                    idle();
                }
            }
        }
    }

    fn scale(&self) -> f32 {
        unsafe { GetDpiForWindow(self.0) as f32 / 96. }
    }

    fn client_size(&self) -> (i32, i32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.0, &mut r);
        }
        (r.right, r.bottom)
    }

    fn is_maximised(&self) -> bool {
        unsafe { IsZoomed(self.0).as_bool() }
    }

    fn set_icon(&self, ico: &Path) {
        unsafe {
            if let Ok(image) = LoadImageW(None, &HSTRING::from(ico.as_os_str()), IMAGE_ICON, 0, 0, LR_LOADFROMFILE | LR_DEFAULTSIZE) {
                SendMessageW(self.0, WM_SETICON, Some(WPARAM(ICON_BIG as usize)), Some(LPARAM(image.0 as isize)));
                SendMessageW(self.0, WM_SETICON, Some(WPARAM(ICON_SMALL as usize)), Some(LPARAM(image.0 as isize)));
            }
        }
    }

    fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.0, SW_SHOW);
        }
    }

    fn to_front(&self) {
        unsafe {
            let _ = SetForegroundWindow(self.0);
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
        unsafe {
            let _ = ShowWindow(self.0, SW_SHOWMAXIMIZED);
        }
        true
    }

    fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.0, SW_HIDE);
        }
    }

    fn close(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.0), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    fn command(&self, command: WindowCommand) {
        let c = match command {
            WindowCommand::Minimise => SC_MINIMIZE,
            WindowCommand::Maximise => SC_MAXIMIZE,
            WindowCommand::Restore => SC_RESTORE,
            WindowCommand::Close => SC_CLOSE,
        };
        unsafe {
            let _ = PostMessageW(Some(self.0), WM_SYSCOMMAND, WPARAM(c as usize), LPARAM(0));
        }
    }

    fn bootstrap(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.0), BOOTSTRAP, WPARAM(0), LPARAM(0));
        }
    }

    fn set_transitions(&self, on: bool) {
        set_attribute(self.0, DWMWA_TRANSITIONS_FORCEDISABLED, &BOOL(i32::from(!on)));
    }

    fn enter_full_screen(&self) -> Option<Restore> {
        unsafe {
            let mut placement = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            let monitor = MonitorFromWindow(self.0, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetWindowPlacement(self.0, &mut placement).is_err() || !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return None;
            }
            SetWindowLongW(self.0, GWL_STYLE, WS_POPUP.0 as i32 | WS_VISIBLE.0 as i32);
            // (none of Windows' frame on the screen's edge: its hairline at the top, its border and its rounding)
            let _ = DwmExtendFrameIntoClientArea(self.0, &windows::Win32::UI::Controls::MARGINS::default());
            set_attribute(self.0, DWMWA_BORDER_COLOR, &0xFFFFFFFEu32);
            set_attribute(self.0, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_DONOTROUND.0);
            let r = info.rcMonitor;
            let _ = SetWindowPos(self.0, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_FRAMECHANGED);
            Some(Restore(placement))
        }
    }

    fn leave_full_screen(&self, restore: &Restore) {
        unsafe {
            SetWindowLongW(self.0, GWL_STYLE, WS_OVERLAPPEDWINDOW.0 as i32 | WS_VISIBLE.0 as i32);
            // (Windows' frame back as the window opened with it: the hairline, the system's border (DWMWA_COLOR_DEFAULT),
            // rounded corners)
            let _ = DwmExtendFrameIntoClientArea(self.0, &windows::Win32::UI::Controls::MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 });
            set_attribute(self.0, DWMWA_BORDER_COLOR, &0xFFFFFFFFu32);
            set_attribute(self.0, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_ROUND.0);
            let _ = SetWindowPlacement(self.0, &restore.0);
            let _ = SetWindowPos(self.0, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
        }
    }

    fn wait_for_refresh(&self) {
        unsafe {
            let _ = DwmFlush();
        }
    }

    fn start_frame_timer(&self) -> bool {
        unsafe { SetTimer(Some(self.0), FRAME_TIMER, 12, None) != 0 }
    }

    fn stop_frame_timer(&self) {
        unsafe {
            let _ = KillTimer(Some(self.0), FRAME_TIMER);
        }
    }

    fn start_ticks(&self) {
        // (nothing on it needs it to the millisecond: Windows may fold it in with other wake-ups)
        unsafe {
            SetCoalescableTimer(Some(self.0), TICK_TIMER, 200, None, 50);
        }
    }

    fn waker(&self) -> Waker {
        Waker(self.0 .0 as isize)
    }

    fn clipboard(&self) -> Option<Vec<u16>> {
        use windows::Win32::System::{DataExchange::*, Memory::*};
        unsafe {
            OpenClipboard(Some(self.0)).ok()?;
            let result = (|| {
                let h = HGLOBAL(GetClipboardData(13).ok()?.0);
                let p = GlobalLock(h) as *const u16;
                if p.is_null() {
                    return None;
                }
                let data = std::slice::from_raw_parts(p, GlobalSize(h) / 2);
                let text = data.iter().take_while(|c| **c != 0).copied().collect();
                let _ = GlobalUnlock(h);
                Some(text)
            })();
            let _ = CloseClipboard();
            result
        }
    }

    fn set_clipboard(&self, text: &[u16]) {
        use windows::Win32::System::{DataExchange::*, Memory::*};
        unsafe {
            if OpenClipboard(Some(self.0)).is_err() {
                return;
            }
            if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, (text.len() + 1) * 2) {
                let p = GlobalLock(h) as *mut u16;
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(text.as_ptr(), p, text.len());
                    let _ = GlobalUnlock(h);
                    if EmptyClipboard().is_ok() && SetClipboardData(13, Some(HANDLE(h.0))).is_ok() {
                        let _ = CloseClipboard();
                        return;
                    }
                }
                let _ = GlobalFree(Some(h));
            }
            let _ = CloseClipboard();
        }
    }

    fn pick_file(&self, kind: &str, patterns: &str) -> Option<String> {
        use windows::Win32::UI::Controls::Dialogs::*;
        let filter: Vec<u16> = format!("{kind}\0{patterns}\0\0").encode_utf16().collect();
        let mut path = [0u16; 32768];
        let mut request = OPENFILENAMEW {
            lStructSize: size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: self.0,
            lpstrFile: PWSTR(path.as_mut_ptr()),
            nMaxFile: path.len() as u32,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        unsafe { GetOpenFileNameW(&mut request).as_bool() }.then(|| String::from_utf16_lossy(&path[..path.iter().position(|c| *c == 0).unwrap_or(path.len())]))
    }
}

/// Asks a zunflix still open to close (its files are about to be replaced), waiting a moment each time.
pub(super) fn close_running_app() {
    unsafe {
        for _ in 0..30 {
            match FindWindowW(w!("ZunflixDesktop"), None) {
                Ok(h) if !h.is_invalid() => {
                    let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                _ => break,
            }
        }
    }
}
