//! Win32 backend: window, GDI blit, input, timing, (later) waveOut.
//! The only file in the project allowed to contain unsafe code (SPEC C8).
//!
//! Field/parameter/type names below intentionally mirror the Win32 API's own
//! casing (hInstance, biWidth, wParam, HWND, BITMAPINFOHEADER, ...) so this
//! file can be cross-referenced directly against MSDN; that's what the
//! blanket allows below are for.
#![allow(non_snake_case)]
#![allow(clippy::upper_case_acronyms)]

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;

// ---------------------------------------------------------------------------
// Minimal hand-rolled Win32 type aliases (no winapi/windows crate).
// ---------------------------------------------------------------------------

type HWND = *mut c_void;
type HDC = *mut c_void;
type HINSTANCE = *mut c_void;
type HMODULE = *mut c_void;
type HICON = *mut c_void;
type HCURSOR = *mut c_void;
type HBRUSH = *mut c_void;
type HMENU = *mut c_void;
type WPARAM = usize;
type LPARAM = isize;
type LRESULT = isize;
type UINT = u32;
type DWORD = u32;
type WORD = u16;
type LONG = i32;
type BOOL = i32;
type LPCWSTR = *const u16;
type LPVOID = *mut c_void;

type WNDPROC = extern "system" fn(HWND, UINT, WPARAM, LPARAM) -> LRESULT;

#[repr(C)]
struct WNDCLASSW {
    style: UINT,
    lpfnWndProc: WNDPROC,
    cbClsExtra: i32,
    cbWndExtra: i32,
    hInstance: HINSTANCE,
    hIcon: HICON,
    hCursor: HCURSOR,
    hbrBackground: HBRUSH,
    lpszMenuName: LPCWSTR,
    lpszClassName: LPCWSTR,
}

#[repr(C)]
struct POINT {
    x: LONG,
    y: LONG,
}

#[repr(C)]
struct MSG {
    hwnd: HWND,
    message: UINT,
    wParam: WPARAM,
    lParam: LPARAM,
    time: DWORD,
    pt: POINT,
}

#[repr(C)]
struct RECT {
    left: LONG,
    top: LONG,
    right: LONG,
    bottom: LONG,
}

#[repr(C)]
struct BITMAPINFOHEADER {
    biSize: DWORD,
    biWidth: LONG,
    biHeight: LONG,
    biPlanes: WORD,
    biBitCount: WORD,
    biCompression: DWORD,
    biSizeImage: DWORD,
    biXPelsPerMeter: LONG,
    biYPelsPerMeter: LONG,
    biClrUsed: DWORD,
    biClrImportant: DWORD,
}

// ---------------------------------------------------------------------------
// Constants (only what's actually used below).
// ---------------------------------------------------------------------------

const CS_VREDRAW: UINT = 0x0001;
const CS_HREDRAW: UINT = 0x0002;
const CS_OWNDC: UINT = 0x0020;

const WS_OVERLAPPEDWINDOW: DWORD = 0x00CF_0000;

const SW_SHOW: i32 = 5;

const PM_REMOVE: UINT = 0x0001;

const WM_DESTROY: UINT = 0x0002;
const WM_CLOSE: UINT = 0x0010;
const WM_PAINT: UINT = 0x000F;
const WM_QUIT: UINT = 0x0012;
const WM_KEYDOWN: UINT = 0x0100;
const WM_KEYUP: UINT = 0x0101;
const WM_SYSKEYDOWN: UINT = 0x0104;
const WM_SYSKEYUP: UINT = 0x0105;

const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;

const IDC_ARROW: usize = 32512;

const BI_RGB: DWORD = 0;
const DIB_RGB_COLORS: UINT = 0;
const SRCCOPY: DWORD = 0x00CC_0020;
const COLORONCOLOR: i32 = 3;

/// Virtual-key code for Escape (main's quit key).
pub const VK_ESCAPE: u8 = 0x1B;

// ---------------------------------------------------------------------------
// extern "system" FFI surface. Only what's used is declared (per SPEC C8).
// ---------------------------------------------------------------------------

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(lpWndClass: *const WNDCLASSW) -> WORD;
    fn CreateWindowExW(
        dwExStyle: DWORD,
        lpClassName: LPCWSTR,
        lpWindowName: LPCWSTR,
        dwStyle: DWORD,
        x: i32,
        y: i32,
        nWidth: i32,
        nHeight: i32,
        hWndParent: HWND,
        hMenu: HMENU,
        hInstance: HINSTANCE,
        lpParam: LPVOID,
    ) -> HWND;
    fn DefWindowProcW(hWnd: HWND, msg: UINT, wParam: WPARAM, lParam: LPARAM) -> LRESULT;
    fn ShowWindow(hWnd: HWND, nCmdShow: i32) -> BOOL;
    fn PeekMessageW(
        lpMsg: *mut MSG,
        hWnd: HWND,
        wMsgFilterMin: UINT,
        wMsgFilterMax: UINT,
        wRemoveMsg: UINT,
    ) -> BOOL;
    fn TranslateMessage(lpMsg: *const MSG) -> BOOL;
    fn DispatchMessageW(lpMsg: *const MSG) -> LRESULT;
    fn PostQuitMessage(nExitCode: i32);
    fn DestroyWindow(hWnd: HWND) -> BOOL;
    fn GetClientRect(hWnd: HWND, lpRect: *mut RECT) -> BOOL;
    fn AdjustWindowRect(lpRect: *mut RECT, dwStyle: DWORD, bMenu: BOOL) -> BOOL;
    fn GetDC(hWnd: HWND) -> HDC;
    fn ReleaseDC(hWnd: HWND, hDC: HDC) -> i32;
    fn ValidateRect(hWnd: HWND, lpRect: *const RECT) -> BOOL;
    fn GetSystemMetrics(nIndex: i32) -> i32;
    fn LoadCursorW(hInstance: HINSTANCE, lpCursorName: LPCWSTR) -> HCURSOR;
}

#[link(name = "gdi32")]
extern "system" {
    #[allow(clippy::too_many_arguments)]
    fn StretchDIBits(
        hdc: HDC,
        xDest: i32,
        yDest: i32,
        DestWidth: i32,
        DestHeight: i32,
        xSrc: i32,
        ySrc: i32,
        SrcWidth: i32,
        SrcHeight: i32,
        lpBits: *const c_void,
        lpbmi: *const BITMAPINFOHEADER,
        iUsage: UINT,
        rop: DWORD,
    ) -> i32;
    fn SetStretchBltMode(hdc: HDC, iStretchMode: i32) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(lpModuleName: LPCWSTR) -> HMODULE;
    fn QueryPerformanceCounter(lpPerformanceCount: *mut i64) -> BOOL;
    fn QueryPerformanceFrequency(lpFrequency: *mut i64) -> BOOL;
    fn Sleep(dwMilliseconds: DWORD);
}

#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(uPeriod: UINT) -> UINT;
    fn timeEndPeriod(uPeriod: UINT) -> UINT;
}

// ---------------------------------------------------------------------------
// Window procedure: only what must be intercepted before poll() sees it.
// Keyboard + all other messages are handled in Platform::poll instead of
// here, so this stays a plain, state-free extern fn (no GWLP_USERDATA).
// ---------------------------------------------------------------------------

extern "system" fn wndproc(hwnd: HWND, msg: UINT, wParam: WPARAM, lParam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            WM_PAINT => {
                // No BeginPaint/EndPaint: we own drawing via blit(), so just
                // clear the update region or PeekMessageW keeps re-delivering
                // WM_PAINT forever.
                ValidateRect(hwnd, ptr::null());
                0
            }
            _ => DefWindowProcW(hwnd, msg, wParam, lParam),
        }
    }
}

fn to_wstring(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------------------
// Public safe API.
// ---------------------------------------------------------------------------

/// Owns the Win32 window, its device context, key state, and timing. All
/// unsafe FFI is contained in this module (SPEC C8); every method below is a
/// safe fn.
pub struct Platform {
    hwnd: HWND,
    hdc: HDC,
    keys: [bool; 256],
    quit: bool,
    qpc_freq: f64,
}

impl Platform {
    /// Registers the window class, creates a resizable overlapped window
    /// whose CLIENT area is exactly `client_w`x`client_h` and centers it on
    /// the primary monitor, then shows it.
    pub fn init(title: &str, client_w: i32, client_h: i32) -> Platform {
        let class_name = to_wstring("FloppySpinWndClass");
        let title_w = to_wstring(title);

        unsafe {
            let hinstance = GetModuleHandleW(ptr::null());

            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW | CS_OWNDC,
                lpfnWndProc: wndproc,
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: ptr::null_mut(),
                hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW as *const u16),
                hbrBackground: ptr::null_mut(),
                lpszMenuName: ptr::null(),
                lpszClassName: class_name.as_ptr(),
            };
            RegisterClassW(&wc);

            // AdjustWindowRect grows a 0,0..client_w,client_h rect to the
            // outer window size needed for that client area under this style.
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: client_w,
                bottom: client_h,
            };
            AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, 0);
            let win_w = rect.right - rect.left;
            let win_h = rect.bottom - rect.top;

            let screen_w = GetSystemMetrics(SM_CXSCREEN);
            let screen_h = GetSystemMetrics(SM_CYSCREEN);
            let x = (screen_w - win_w) / 2;
            let y = (screen_h - win_h) / 2;

            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title_w.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                x,
                y,
                win_w,
                win_h,
                ptr::null_mut(),
                ptr::null_mut(),
                hinstance,
                ptr::null_mut(),
            );

            ShowWindow(hwnd, SW_SHOW);

            // CS_OWNDC gives this window a private DC we can hold for its
            // whole lifetime instead of Get/Release around every blit.
            let hdc = GetDC(hwnd);
            SetStretchBltMode(hdc, COLORONCOLOR);

            timeBeginPeriod(1);

            let mut freq: i64 = 0;
            QueryPerformanceFrequency(&mut freq);

            Platform {
                hwnd,
                hdc,
                keys: [false; 256],
                quit: false,
                qpc_freq: freq as f64,
            }
        }
    }

    /// Drains the message queue, updating key state from WM_KEYDOWN/UP and
    /// WM_SYSKEYDOWN/UP. Returns `false` once WM_CLOSE/WM_DESTROY has posted
    /// WM_QUIT (the caller should stop calling poll and exit).
    pub fn poll(&mut self) -> bool {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                match msg.message {
                    WM_QUIT => self.quit = true,
                    WM_KEYDOWN | WM_SYSKEYDOWN => {
                        self.keys[(msg.wParam & 0xFF) as usize] = true;
                    }
                    WM_KEYUP | WM_SYSKEYUP => {
                        self.keys[(msg.wParam & 0xFF) as usize] = false;
                    }
                    _ => {}
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        !self.quit
    }

    /// Current down/up state of virtual-key `vk`.
    pub fn key(&self, vk: u8) -> bool {
        self.keys[vk as usize]
    }

    /// Stretches the 0x00RRGGBB `fb` (top-down, `fb_w`x`fb_h`) onto the
    /// window's current client rect (queried fresh each call since the
    /// window is resizable).
    pub fn blit(&mut self, fb: &[u32], fb_w: i32, fb_h: i32) {
        assert_eq!(fb.len(), (fb_w * fb_h) as usize);
        unsafe {
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(self.hwnd, &mut rect);
            let win_w = rect.right - rect.left;
            let win_h = rect.bottom - rect.top;

            let bmih = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: fb_w,
                biHeight: -fb_h, // negative => top-down DIB
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };

            StretchDIBits(
                self.hdc,
                0,
                0,
                win_w,
                win_h,
                0,
                0,
                fb_w,
                fb_h,
                fb.as_ptr() as *const c_void,
                &bmih,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
        }
    }

    /// Seconds since an arbitrary epoch, from QueryPerformanceCounter.
    pub fn now_s(&self) -> f64 {
        let mut counter: i64 = 0;
        unsafe {
            QueryPerformanceCounter(&mut counter);
        }
        counter as f64 / self.qpc_freq
    }

    /// Coarse OS sleep (kernel32 Sleep). Pair with a spin-wait for the last
    /// sub-millisecond stretch when pacing frames.
    pub fn sleep_ms(ms: u32) {
        unsafe {
            Sleep(ms);
        }
    }
}

impl Drop for Platform {
    fn drop(&mut self) {
        unsafe {
            timeEndPeriod(1);
            ReleaseDC(self.hwnd, self.hdc);
            DestroyWindow(self.hwnd);
        }
    }
}
