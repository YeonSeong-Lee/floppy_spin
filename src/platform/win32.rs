//! Win32 backend: window, GDI blit, input, timing, waveOut ring (Task M6-B).
//! The only file in the project allowed to contain unsafe code (SPEC C8).
//!
//! Field/parameter/type names below intentionally mirror the Win32 API's own
//! casing (hInstance, biWidth, wParam, HWND, BITMAPINFOHEADER, ...) so this
//! file can be cross-referenced directly against MSDN; that's what the
//! blanket allows below are for.
#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
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
/// `LONG_PTR`: pointer-sized signed integer, i.e. `isize` on the x86_64 ABI
/// this project targets (SPEC C2) — used only by `Get/SetWindowLongPtrW`'s
/// `GWL_STYLE` value below.
type LONG_PTR = isize;

// ---- waveOut (Task M6-B) ---------------------------------------------------
/// Opaque waveOut device handle.
type HWAVEOUT = *mut c_void;
/// `UINT_PTR`/`DWORD_PTR`: pointer-sized on every ABI winmm.dll ships for,
/// which on `x86_64-pc-windows-gnu` (SPEC C2) is exactly `usize`.
type UINT_PTR = usize;
type DWORD_PTR = usize;
/// `MMRESULT` is a plain `UINT` return code (`MMSYSERR_*`/`WAVERR_*`).
type MMRESULT = u32;

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

/// `WAVEFORMATEX` (winmm.h), PCM case (`cbSize` unused, kept 0).
#[repr(C)]
struct WAVEFORMATEX {
    wFormatTag: WORD,
    nChannels: WORD,
    nSamplesPerSec: DWORD,
    nAvgBytesPerSec: DWORD,
    nBlockAlign: WORD,
    wBitsPerSample: WORD,
    cbSize: WORD,
}

/// `WAVEHDR` (winmm.h). Its address must stay stable for as long as it is
/// `WHDR_PREPARED`/queued — see `AudioBuffer`'s doc comment for how this file
/// guarantees that.
#[repr(C)]
struct WAVEHDR {
    lpData: *mut u8,
    dwBufferLength: DWORD,
    dwBytesRecorded: DWORD,
    dwUser: DWORD_PTR,
    dwFlags: DWORD,
    dwLoops: DWORD,
    lpNext: *mut WAVEHDR,
    reserved: DWORD_PTR,
}

// ---------------------------------------------------------------------------
// Constants (only what's actually used below).
// ---------------------------------------------------------------------------

const CS_VREDRAW: UINT = 0x0001;
const CS_HREDRAW: UINT = 0x0002;
const CS_OWNDC: UINT = 0x0020;

const WS_OVERLAPPEDWINDOW: DWORD = 0x00CF_0000;
const WS_POPUP: DWORD = 0x8000_0000;
const WS_VISIBLE: DWORD = 0x1000_0000;

const SW_SHOW: i32 = 5;

// ---- window-scale resize (SPEC §7: "window scale (1x/1.5x/2x/borderless-
// fullscreen; internal resolution is always 960x540)") -----------------------

const GWL_STYLE: i32 = -16;
const HWND_TOP: HWND = ptr::null_mut(); // HWND_TOP == 0
const SWP_NOZORDER: UINT = 0x0004;
const SWP_FRAMECHANGED: UINT = 0x0020;
const SWP_SHOWWINDOW: UINT = 0x0040;

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

// ---- waveOut (Task M6-B; SPEC §8) ------------------------------------------

/// `(UINT)-1` widened to `UINT_PTR`: "let the driver pick the default
/// device", passed as `waveOutOpen`'s device-ID argument.
const WAVE_MAPPER: UINT_PTR = 0xFFFF_FFFF;
/// No callback function/window/thread/event (HARD RULES: single-threaded,
/// polling only — `WHDR_DONE` is checked from the main loop, never a
/// callback).
const CALLBACK_NULL: DWORD = 0x0000_0000;
const WAVE_FORMAT_PCM: WORD = 1;
const MMSYSERR_NOERROR: MMRESULT = 0;
/// Set once `waveOutWrite` has finished playing a buffer — this is the flag
/// `AudioRing::free_count`/`submit` poll from the main loop (HARD RULES: no
/// callback, no event handle; plain polling).
const WHDR_DONE: DWORD = 0x0000_0001;

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
    // ---- window-scale resize (SPEC §7) ----
    #[allow(clippy::too_many_arguments)]
    fn SetWindowPos(
        hWnd: HWND,
        hWndInsertAfter: HWND,
        X: i32,
        Y: i32,
        cx: i32,
        cy: i32,
        uFlags: UINT,
    ) -> BOOL;
    fn GetWindowLongPtrW(hWnd: HWND, nIndex: i32) -> LONG_PTR;
    fn SetWindowLongPtrW(hWnd: HWND, nIndex: i32, dwNewLong: LONG_PTR) -> LONG_PTR;
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
    // ---- Save I/O (Task M8-3; SPEC §9) ----
    fn GetEnvironmentVariableW(lpName: LPCWSTR, lpBuffer: *mut u16, nSize: DWORD) -> DWORD;
    fn CreateDirectoryW(lpPathName: LPCWSTR, lpSecurityAttributes: LPVOID) -> BOOL;
}

#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(uPeriod: UINT) -> UINT;
    fn timeEndPeriod(uPeriod: UINT) -> UINT;

    fn waveOutOpen(
        phwo: *mut HWAVEOUT,
        uDeviceID: UINT_PTR,
        pwfx: *const WAVEFORMATEX,
        dwCallback: DWORD_PTR,
        dwInstance: DWORD_PTR,
        fdwOpen: DWORD,
    ) -> MMRESULT;
    fn waveOutPrepareHeader(hwo: HWAVEOUT, pwh: *mut WAVEHDR, cbwh: UINT) -> MMRESULT;
    fn waveOutUnprepareHeader(hwo: HWAVEOUT, pwh: *mut WAVEHDR, cbwh: UINT) -> MMRESULT;
    fn waveOutWrite(hwo: HWAVEOUT, pwh: *mut WAVEHDR, cbwh: UINT) -> MMRESULT;
    fn waveOutReset(hwo: HWAVEOUT) -> MMRESULT;
    fn waveOutClose(hwo: HWAVEOUT) -> MMRESULT;
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
// waveOut ring (Task M6-B; SPEC §8 / HARD RULES).
//
// Design: `AUDIO_RING_BUFFERS` (4) buffers of `AUDIO_BUFFER_FRAMES` (1024)
// stereo frames each (~23.2ms/buffer @44.1kHz => ~93ms worst-case queued
// latency), each `waveOutPrepareHeader`'d exactly ONCE at `AudioRing::open`
// and then recycled forever via repeated `waveOutWrite` calls on the same
// still-prepared `WAVEHDR` (a standard waveOut ring technique — re-preparing
// per write is unnecessary and would be wasted work every frame). No
// callback, no dedicated thread, no event handle (CALLBACK_NULL): the main
// loop polls each header's `WHDR_DONE` bit once a frame and only refills/
// requeues buffers that have actually finished playing.
//
// Buffer memory (`AudioBuffer::data`, a `Vec<i16>`) is heap-allocated once at
// `AudioRing::open` and never resized afterward, so the raw pointer stashed
// in `hdr.lpData` stays valid even though the owning `AudioBuffer`/`AudioRing`
// /`Platform` values themselves may move (moving a `Vec`'s `(ptr,len,cap)`
// header does not move its heap-allocated backing storage).
// ---------------------------------------------------------------------------

/// Ring buffer count (module docs above).
pub const AUDIO_RING_BUFFERS: usize = 4;
/// Stereo FRAMES (one L+R sample pair) per ring buffer (module docs above).
pub const AUDIO_BUFFER_FRAMES: usize = 1024;

const AUDIO_CHANNELS: WORD = 2;
const AUDIO_BITS_PER_SAMPLE: WORD = 16;
/// Fixed mono sample rate (SPEC §8; mirrors `floppy_audio::SAMPLE_RATE`).
/// Hardcoded rather than threaded through from that crate so this
/// unsafe-only file stays a plain wire-format layer, not an `floppy_audio`
/// API consumer.
const AUDIO_SAMPLE_RATE: DWORD = 44_100;

/// One ring slot: its sample storage plus the `WAVEHDR` Windows keeps a
/// pointer to while queued. `queued` tracks "has this buffer ever been
/// submitted" — needed because a fresh, never-written buffer has `dwFlags ==
/// WHDR_PREPARED` (no `WHDR_DONE` bit) despite also being available, and the
/// free-space test below needs to treat both cases as "free" alike.
struct AudioBuffer {
    data: Vec<i16>,
    hdr: WAVEHDR,
    queued: bool,
}

impl AudioBuffer {
    fn is_free(&self) -> bool {
        !self.queued || (self.hdr.dwFlags & WHDR_DONE) != 0
    }
}

/// The waveOut playback ring. `hwo.is_null()` means "no audio device" (either
/// never opened or `waveOutOpen` failed) — every method is then a silent
/// no-op, so the game still runs (silently) rather than crashing or hanging
/// (module brief: graceful degradation).
struct AudioRing {
    hwo: HWAVEOUT,
    buffers: Vec<AudioBuffer>,
}

impl AudioRing {
    /// The not-yet-opened state: `Platform::init` constructs this; nothing
    /// touches winmm until `Platform::audio_init` is called.
    fn closed() -> AudioRing {
        AudioRing {
            hwo: ptr::null_mut(),
            buffers: Vec::new(),
        }
    }

    /// Opens the default waveOut device at [`AUDIO_SAMPLE_RATE`]/16-bit/
    /// stereo and prepares [`AUDIO_RING_BUFFERS`] buffers once each. Any
    /// failure (no device, format rejected, ...) yields the same `closed()`
    /// no-op state instead of propagating an error — there is no sim-visible
    /// consequence to running with audio off (HARD RULES: audio never writes
    /// anything sim-visible).
    fn open() -> AudioRing {
        let block_align = AUDIO_CHANNELS * (AUDIO_BITS_PER_SAMPLE / 8);
        let wfx = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM,
            nChannels: AUDIO_CHANNELS,
            nSamplesPerSec: AUDIO_SAMPLE_RATE,
            nAvgBytesPerSec: AUDIO_SAMPLE_RATE * block_align as DWORD,
            nBlockAlign: block_align,
            wBitsPerSample: AUDIO_BITS_PER_SAMPLE,
            cbSize: 0,
        };

        let mut hwo: HWAVEOUT = ptr::null_mut();
        let result = unsafe { waveOutOpen(&mut hwo, WAVE_MAPPER, &wfx, 0, 0, CALLBACK_NULL) };
        if result != MMSYSERR_NOERROR || hwo.is_null() {
            return AudioRing::closed();
        }

        // Build every buffer FIRST, then prepare each header at its final
        // heap address (M5/M6 verifier finding: preparing a stack-local
        // WAVEHDR and moving it afterward worked in practice because the
        // driver only retains lpData, but prepare-then-move is not the
        // textbook-safe ordering). The capacity is exact and nothing pushes
        // later, so the Vec never reallocates and the addresses are
        // permanent for the ring's whole life.
        let mut buffers = Vec::with_capacity(AUDIO_RING_BUFFERS);
        for _ in 0..AUDIO_RING_BUFFERS {
            let mut data = vec![0i16; AUDIO_BUFFER_FRAMES * 2];
            let hdr = WAVEHDR {
                lpData: data.as_mut_ptr() as *mut u8,
                dwBufferLength: (data.len() * size_of::<i16>()) as DWORD,
                dwBytesRecorded: 0,
                dwUser: 0,
                dwFlags: 0,
                dwLoops: 0,
                lpNext: ptr::null_mut(),
                reserved: 0,
            };
            buffers.push(AudioBuffer {
                data,
                hdr,
                queued: false,
            });
        }
        for buf in &mut buffers {
            let result =
                unsafe { waveOutPrepareHeader(hwo, &mut buf.hdr, size_of::<WAVEHDR>() as UINT) };
            if result != MMSYSERR_NOERROR {
                // Degrade to the silent no-device path rather than running a
                // ring with a half-prepared header set: unprepare whatever
                // did prepare (unpreparing a never-prepared header is safe —
                // winmm no-ops on the missing WHDR_PREPARED flag), close,
                // and report "no audio".
                unsafe {
                    waveOutReset(hwo);
                    for b in &mut buffers {
                        waveOutUnprepareHeader(hwo, &mut b.hdr, size_of::<WAVEHDR>() as UINT);
                    }
                    waveOutClose(hwo);
                }
                return AudioRing::closed();
            }
        }

        AudioRing { hwo, buffers }
    }

    /// Ring slots currently free to accept a fresh `AUDIO_BUFFER_FRAMES`-long
    /// mono chunk (0 with no audio device).
    fn free_count(&self) -> usize {
        self.buffers.iter().filter(|b| b.is_free()).count()
    }

    /// Duplicate `mono` (must be exactly `AUDIO_BUFFER_FRAMES` samples; SPEC
    /// §8: "mono mixer output duplicated L=R at submit time") into the next
    /// free ring slot and queue it with `waveOutWrite`. A silent no-op if
    /// there is no free slot (the caller is expected to have checked
    /// `free_count` first) or no audio device.
    fn submit(&mut self, mono: &[i16]) {
        if self.hwo.is_null() {
            return;
        }
        debug_assert_eq!(mono.len(), AUDIO_BUFFER_FRAMES);
        let Some(buf) = self.buffers.iter_mut().find(|b| b.is_free()) else {
            return;
        };
        for (i, &s) in mono.iter().enumerate() {
            buf.data[i * 2] = s;
            buf.data[i * 2 + 1] = s;
        }
        buf.queued = true;
        let result = unsafe { waveOutWrite(self.hwo, &mut buf.hdr, size_of::<WAVEHDR>() as UINT) };
        if result != MMSYSERR_NOERROR {
            // The driver rejected the write, so WHDR_DONE will never arrive
            // for this header; un-mark the slot or it would stay "queued"
            // forever and silently shrink the ring (M5/M6 verifier finding).
            // The chunk itself is dropped — one lost ~23 ms of audio beats a
            // permanently stranded buffer.
            buf.queued = false;
        }
    }

    /// `waveOutReset` (stop + mark every buffer done) -> `UnprepareHeader`
    /// each buffer -> `waveOutClose`, in that order — the order the Windows
    /// docs prescribe to avoid a hang (module brief). A no-op with no device.
    fn shutdown(&mut self) {
        if self.hwo.is_null() {
            return;
        }
        unsafe {
            waveOutReset(self.hwo);
            for buf in &mut self.buffers {
                waveOutUnprepareHeader(self.hwo, &mut buf.hdr, size_of::<WAVEHDR>() as UINT);
            }
            waveOutClose(self.hwo);
        }
        self.hwo = ptr::null_mut();
    }
}

// ---------------------------------------------------------------------------
// Window-scale resize (SPEC §7: "window scale (1x/1.5x/2x/borderless-
// fullscreen; internal resolution is always 960x540)"). Purely a WINDOW
// sizing concern — the internal render buffer stays 960x540 always; `blit`'s
// `StretchDIBits` (already `SetStretchBltMode(COLORONCOLOR)`) is what scales
// that fixed framebuffer up to whatever the client area ends up being, so
// nothing here touches the sim or the render buffer.
// ---------------------------------------------------------------------------

/// Platform-layer window-scale mode (deliberately not `floppy_core::flow`'s
/// `WindowScale` — this module must not depend on `floppy_core`; `main.rs`
/// maps one to the other).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowScaleMode {
    X1,
    X1_5,
    X2,
    Fullscreen,
}

/// Internal render resolution (SPEC §7), used to compute the client size for
/// each windowed scale tier.
const BASE_W: i32 = 960;
const BASE_H: i32 = 540;

// ---------------------------------------------------------------------------
// Public safe API.
// ---------------------------------------------------------------------------

/// Owns the Win32 window, its device context, key state, timing, and the
/// waveOut playback ring. All unsafe FFI is contained in this module (SPEC
/// C8); every method below is a safe fn.
pub struct Platform {
    hwnd: HWND,
    hdc: HDC,
    keys: [bool; 256],
    quit: bool,
    qpc_freq: f64,
    audio: AudioRing,
    /// Last-applied window-scale mode, so re-applying the same mode every
    /// time the settings screen is exited (even with no change) is a cheap
    /// no-op instead of thrashing the window (task brief).
    window_scale: WindowScaleMode,
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
                audio: AudioRing::closed(),
                // `init`'s window is always created windowed at exactly
                // `client_w`x`client_h`, which is the X1 (960x540) case for
                // every current caller (`main.rs`) — kept as the starting
                // point so a boot-time `set_window_scale(X1)` call is
                // correctly recognized as a no-op.
                window_scale: WindowScaleMode::X1,
            }
        }
    }

    /// Opens the waveOut ring (module docs above `AudioRing`). Call once
    /// after `init`. Safe to call even if no audio device is present — the
    /// ring degrades to a silent no-op path and the game still runs.
    pub fn audio_init(&mut self) {
        self.audio = AudioRing::open();
    }

    /// Ring slots currently free to accept a fresh [`AUDIO_BUFFER_FRAMES`]
    /// -long mono chunk (0 if `audio_init` was never called, or no device).
    pub fn audio_free_buffers(&mut self) -> usize {
        self.audio.free_count()
    }

    /// Submit one [`AUDIO_BUFFER_FRAMES`]-length mono chunk: duplicated to
    /// interleaved stereo and queued via `waveOutWrite` (SPEC §8). No-op if
    /// there is no free ring slot or no audio device.
    pub fn audio_submit(&mut self, mono: &[i16]) {
        self.audio.submit(mono);
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

    /// Applies a window-scale mode (SPEC §7): resizes/restyles the WINDOW
    /// only — the internal 960x540 render buffer never changes, and neither
    /// does `blit`'s `StretchDIBits` call, which already stretches whatever
    /// the client rect turns out to be. A no-op if `mode` is already the
    /// current mode (so calling this on every settings-exit, or once at
    /// boot with the default X1, never thrashes the window), and a silent
    /// no-op on any FFI failure — a botched resize must never crash the game
    /// (mirrors the waveOut graceful-degradation posture elsewhere in this
    /// file).
    pub fn set_window_scale(&mut self, mode: WindowScaleMode) {
        if mode == self.window_scale {
            return;
        }
        if self.hwnd.is_null() {
            return;
        }

        unsafe {
            // Read the current style so the (comparatively heavier, and
            // SWP_FRAMECHANGED-requiring) SetWindowLongPtrW call only
            // happens when the target style actually differs from the
            // current one, rather than unconditionally on every call.
            let current_style = GetWindowLongPtrW(self.hwnd, GWL_STYLE) as DWORD;

            match mode {
                WindowScaleMode::Fullscreen => {
                    let style = WS_POPUP | WS_VISIBLE;
                    if current_style != style {
                        SetWindowLongPtrW(self.hwnd, GWL_STYLE, style as LONG_PTR);
                    }

                    let screen_w = GetSystemMetrics(SM_CXSCREEN);
                    let screen_h = GetSystemMetrics(SM_CYSCREEN);
                    SetWindowPos(
                        self.hwnd,
                        HWND_TOP,
                        0,
                        0,
                        screen_w,
                        screen_h,
                        SWP_NOZORDER | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
                    );
                }
                WindowScaleMode::X1 | WindowScaleMode::X1_5 | WindowScaleMode::X2 => {
                    let scale = match mode {
                        WindowScaleMode::X1 => 1.0,
                        WindowScaleMode::X1_5 => 1.5,
                        WindowScaleMode::X2 => 2.0,
                        WindowScaleMode::Fullscreen => unreachable!(),
                    };
                    let client_w = (BASE_W as f32 * scale).round() as i32;
                    let client_h = (BASE_H as f32 * scale).round() as i32;

                    // Restore the normal overlapped-window style (skipped if
                    // it's already that style, e.g. switching X1 -> X2).
                    if current_style != WS_OVERLAPPEDWINDOW {
                        SetWindowLongPtrW(self.hwnd, GWL_STYLE, WS_OVERLAPPEDWINDOW as LONG_PTR);
                    }

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

                    SetWindowPos(
                        self.hwnd,
                        HWND_TOP,
                        x,
                        y,
                        win_w,
                        win_h,
                        SWP_NOZORDER | SWP_FRAMECHANGED | SWP_SHOWWINDOW,
                    );
                }
            }
        }
        self.window_scale = mode;
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
        // Tear down audio first (module docs on `AudioRing::shutdown`: reset
        // -> unprepare -> close, avoids a driver hang) — independent of the
        // window teardown below, order between the two doesn't matter.
        self.audio.shutdown();
        unsafe {
            timeEndPeriod(1);
            ReleaseDC(self.hwnd, self.hdc);
            DestroyWindow(self.hwnd);
        }
    }
}

// ---------------------------------------------------------------------------
// Save I/O (Task M8-3; SPEC §9): `%APPDATA%\floppy_spin\save.bin`. The only
// unsafe here is fetching the `%APPDATA%` path (`GetEnvironmentVariableW`,
// since that's not a std-portable env read the rest of the codebase uses)
// and creating the `floppy_spin` subdirectory (`CreateDirectoryW`); the
// actual file read/write goes through `std::fs`, which is plain safe Rust
// and is allowed in this root-level unsafe-only file per the task brief
// ("std::fs is allowed here in the root bin ... simplest: GetEnvironmentVariableW
// for the path, then std::fs::write"). Both directions are best-effort: any
// failure (missing var, missing file, a locked/unwritable directory, ...)
// degrades to "no save" rather than ever panicking or crashing the game
// (SPEC §9: "never crash").
// ---------------------------------------------------------------------------

/// `%APPDATA%` env var name (SPEC §9: "path via GetEnvironmentVariableW").
const APPDATA_VAR: &str = "APPDATA";
/// Save file location under `%APPDATA%` (SPEC §9, exact).
const SAVE_SUBPATH: &str = "floppy_spin\\save.bin";
const SAVE_DIR_SUBPATH: &str = "floppy_spin";

/// Read `%APPDATA%` via `GetEnvironmentVariableW`. Returns `None` if the
/// variable is unset or the call otherwise fails (buffer-too-small is
/// handled by growing once to the size the API reports it needs).
fn appdata_dir() -> Option<String> {
    let name = to_wstring(APPDATA_VAR);
    let mut buf: Vec<u16> = vec![0; 512];
    let len =
        unsafe { GetEnvironmentVariableW(name.as_ptr(), buf.as_mut_ptr(), buf.len() as DWORD) };
    if len == 0 {
        return None; // unset, or a genuine failure — both mean "no save".
    }
    if len as usize > buf.len() {
        // The initial 512-u16 buffer was too small; `len` is the required
        // size (including the nul terminator) — retry once at that size.
        buf = vec![0; len as usize];
        let len2 =
            unsafe { GetEnvironmentVariableW(name.as_ptr(), buf.as_mut_ptr(), buf.len() as DWORD) };
        if len2 == 0 || len2 as usize > buf.len() {
            return None;
        }
        buf.truncate(len2 as usize);
    } else {
        buf.truncate(len as usize);
    }
    String::from_utf16(&buf).ok()
}

/// Read the whole save file. Returns an empty `Vec` on ANY failure (no
/// `%APPDATA%`, no directory, no file, a read error, ...) — `save::decode`
/// turns an empty (or any otherwise-invalid) blob into `SaveState::default()`,
/// so this never needs to distinguish "missing" from "corrupt" itself.
pub fn save_load() -> Vec<u8> {
    let Some(appdata) = appdata_dir() else {
        return Vec::new();
    };
    let path = std::path::Path::new(&appdata).join(SAVE_SUBPATH);
    std::fs::read(path).unwrap_or_default()
}

/// Best-effort save write: create `%APPDATA%\floppy_spin` if missing (via
/// `CreateDirectoryW`; "already exists" is not an error — ignored the same
/// as any other failure here), then `std::fs::write` the bytes. Never
/// panics, never propagates an error — a failed save is silently dropped
/// (task brief: "never crash the game over a save").
pub fn save_store(bytes: &[u8]) {
    let Some(appdata) = appdata_dir() else {
        return;
    };
    let dir = std::path::Path::new(&appdata).join(SAVE_DIR_SUBPATH);
    let dir_w = to_wstring(&dir.to_string_lossy());
    unsafe {
        // Return value ignored on purpose: 0 covers both "already exists"
        // (ERROR_ALREADY_EXISTS) and any other failure; either way the
        // subsequent write attempt is the real success/failure signal.
        CreateDirectoryW(dir_w.as_ptr(), ptr::null_mut());
    }
    let path = dir.join("save.bin");
    let _ = std::fs::write(path, bytes);
}
