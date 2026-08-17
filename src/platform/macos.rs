//! macOS dev backend: window, blit, input, timing, audio ring (SPEC §2 decision
//! record; ROADMAP "deferred / stretch").
//!
//! This is the counterpart to [`super::win32`] and exposes the **exact same**
//! public API, so `main.rs` is a single non-cfg-split source file that both
//! platforms compile. That symmetry is the point: `main.rs` compiling here is
//! evidence it still compiles there.
//!
//! Unlike `win32.rs` this file contains no `unsafe` and no FFI — it is safe
//! Rust over `winit` + `softbuffer` + `cpal`, which are gated behind
//! `[target.'cfg(target_os = "macos")'.dependencies]` in the root manifest and
//! therefore never enter the shipped Windows build graph, its import allowlist
//! (SPEC §12.3), or its size budget (C1). The `forbid` below enforces the
//! "safe backend" wording in SPEC §4 rather than merely asserting it.
//!
//! Best-effort by construction (SPEC §2): this backend is a development
//! convenience for working on the game away from Windows. It is **not** a ship
//! gate — `floppy_spin.exe` is the only shipped artifact, and every §12 gate
//! that judges it still runs on Windows.
#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::platform::pump_events::{EventLoopExtPumpEvents, PumpStatus};
use winit::window::{Fullscreen, Window, WindowId};

// ---------------------------------------------------------------------------
// Constants mirrored from `win32.rs` (same names, same values — see module
// docs on why the two backends must stay API-identical).
// ---------------------------------------------------------------------------

/// Virtual-key code for Escape (main's quit key). Windows VK codes are the
/// lingua franca between `main.rs` and either backend; this file translates
/// winit's `KeyCode` into them (see [`vk_for`]) so `main.rs` needs no cfg.
pub const VK_ESCAPE: u8 = 0x1B;

/// Number of in-flight audio chunks, matching `win32`'s waveOut ring depth.
pub const AUDIO_RING_BUFFERS: usize = 4;
/// Mono samples per submitted chunk (~23 ms at 44.1 kHz).
pub const AUDIO_BUFFER_FRAMES: usize = 1024;
/// Fixed mono sample rate (SPEC §8; mirrors `floppy_audio::SAMPLE_RATE` and
/// `win32`'s `AUDIO_SAMPLE_RATE`). Requested explicitly rather than taking the
/// device default, which on CoreAudio is usually 48 kHz and would transpose
/// every synthesized note.
const AUDIO_SAMPLE_RATE: u32 = 44_100;

/// Total mono samples the ring will hold before [`AudioRing::free_count`]
/// reports "full".
const AUDIO_RING_CAPACITY: usize = AUDIO_RING_BUFFERS * AUDIO_BUFFER_FRAMES;

/// Internal render resolution (SPEC §7), used to compute the client size for
/// each windowed scale tier.
const BASE_W: u32 = 960;
const BASE_H: u32 = 540;

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

/// Windowed client size for a scale tier, in *logical* points. `win32` sizes
/// the client rect in physical pixels; on a Retina display that would produce
/// a window a quarter of the expected area, so the macOS side sizes logically
/// and lets the upscale in [`Platform::blit`] cover the extra device pixels.
fn client_size_for(mode: WindowScaleMode) -> (u32, u32) {
    match mode {
        WindowScaleMode::X1 | WindowScaleMode::Fullscreen => (BASE_W, BASE_H),
        WindowScaleMode::X1_5 => (BASE_W * 3 / 2, BASE_H * 3 / 2),
        WindowScaleMode::X2 => (BASE_W * 2, BASE_H * 2),
    }
}

/// winit physical key -> Windows virtual-key code, for exactly the keys
/// `main.rs` polls (arrows, Space, Shift, Ctrl, Z/X/C, Esc) and nothing else.
/// Both sides of a modifier pair map to the single Windows VK, matching
/// `win32`'s `WM_KEYDOWN` behaviour where `VK_SHIFT`/`VK_CONTROL` are
/// side-agnostic.
fn vk_for(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::ArrowLeft => 0x25,
        KeyCode::ArrowUp => 0x26,
        KeyCode::ArrowRight => 0x27,
        KeyCode::ArrowDown => 0x28,
        KeyCode::Space => 0x20,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => 0x10,
        KeyCode::ControlLeft | KeyCode::ControlRight => 0x11,
        KeyCode::KeyZ => 0x5A,
        KeyCode::KeyX => 0x58,
        KeyCode::KeyC => 0x43,
        KeyCode::Escape => VK_ESCAPE,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Audio: cpal output stream fed from a plain mono ring.
//
// `win32` hands waveOut N prepared headers and polls WHDR_DONE to learn how
// many are free. cpal instead pulls from a callback on its own audio thread,
// so the equivalent "how much can I push" signal is the free space in a
// shared queue. Chunk accounting is kept identical (whole
// AUDIO_BUFFER_FRAMES-sized chunks) so `main.rs`'s submit loop behaves the
// same on both backends.
// ---------------------------------------------------------------------------

/// Lock helper that survives a poisoned mutex. A panic in the audio callback
/// would poison the queue; the game should fall back to silence-ish audio, not
/// die, so poisoning is treated as "use the data anyway".
fn lock_queue(q: &Mutex<VecDeque<i16>>) -> MutexGuard<'_, VecDeque<i16>> {
    q.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whole chunks a queue holding `len` samples can still accept. Split out from
/// [`AudioRing::free_count`] so the accounting is testable without an audio
/// device (there is none in CI, and none in a headless run).
fn free_chunks(len: usize) -> usize {
    AUDIO_RING_CAPACITY.saturating_sub(len) / AUDIO_BUFFER_FRAMES
}

/// Append one chunk if it fits whole, and report whether it did. A chunk that
/// would overflow is dropped rather than truncated: a half-written chunk would
/// desync the mono stream against `main.rs`'s per-frame accounting, and one
/// dropped ~23 ms chunk is inaudible next to that.
fn push_chunk(queue: &mut VecDeque<i16>, mono: &[i16]) -> bool {
    let fits = queue.len() + mono.len() <= AUDIO_RING_CAPACITY;
    if fits {
        queue.extend(mono.iter().copied());
    }
    fits
}

struct AudioRing {
    queue: Arc<Mutex<VecDeque<i16>>>,
    /// `None` until [`AudioRing::open`] succeeds, and on any machine with no
    /// usable output device — the whole ring then degrades to a no-op and the
    /// game runs silent rather than failing to start (same contract as
    /// `win32`'s).
    stream: Option<cpal::Stream>,
}

impl AudioRing {
    fn silent() -> AudioRing {
        AudioRing {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            stream: None,
        }
    }

    /// Opens the default output device at [`AUDIO_SAMPLE_RATE`], f32 samples,
    /// mono duplicated across however many channels the device wants. Any
    /// failure returns a silent ring.
    fn open() -> AudioRing {
        let queue: Arc<Mutex<VecDeque<i16>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(AUDIO_RING_CAPACITY)));

        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            eprintln!("[macos] no default audio output device — running silent");
            return AudioRing::silent();
        };

        // Prefer an f32 config that actually supports 44.1 kHz; CoreAudio
        // offers f32 on every real device. Falling back to the device default
        // would transpose the whole soundtrack, so a miss means silence
        // instead — this is a dev backend, and a wrong-pitch mix is worse than
        // none for judging the synth.
        let Some(supported) = device
            .supported_output_configs()
            .ok()
            .and_then(|mut cfgs| {
                cfgs.find(|c| {
                    c.sample_format() == cpal::SampleFormat::F32
                        && c.min_sample_rate().0 <= AUDIO_SAMPLE_RATE
                        && c.max_sample_rate().0 >= AUDIO_SAMPLE_RATE
                        && c.channels() >= 1
                })
            })
            .map(|c| c.with_sample_rate(cpal::SampleRate(AUDIO_SAMPLE_RATE)))
        else {
            eprintln!("[macos] no f32 output config at {AUDIO_SAMPLE_RATE} Hz — running silent");
            return AudioRing::silent();
        };

        let channels = supported.channels() as usize;
        let config: cpal::StreamConfig = supported.config();
        let cb_queue = Arc::clone(&queue);

        let stream = device.build_output_stream(
            &config,
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let mut q = lock_queue(&cb_queue);
                for frame in out.chunks_mut(channels) {
                    // Underrun fills silence rather than repeating the last
                    // sample: a gap is easier to hear (and diagnose) than a
                    // buzz, and `main.rs` tops the ring up every frame.
                    let s = q.pop_front().unwrap_or(0) as f32 / 32_768.0;
                    for ch in frame.iter_mut() {
                        *ch = s;
                    }
                }
            },
            |err| eprintln!("[macos] audio stream error: {err}"),
            None,
        );

        match stream {
            Ok(stream) => {
                if let Err(e) = stream.play() {
                    eprintln!("[macos] could not start audio stream: {e} — running silent");
                    return AudioRing::silent();
                }
                AudioRing {
                    queue,
                    stream: Some(stream),
                }
            }
            Err(e) => {
                eprintln!("[macos] could not open audio stream: {e} — running silent");
                AudioRing::silent()
            }
        }
    }

    /// Whole [`AUDIO_BUFFER_FRAMES`]-sized chunks the queue can still accept.
    fn free_count(&mut self) -> usize {
        if self.stream.is_none() {
            return 0;
        }
        free_chunks(lock_queue(&self.queue).len())
    }

    /// Queue one mono chunk. Dropped (not truncated) if it would overflow the
    /// ring, so the queue never holds a partial chunk.
    fn submit(&mut self, mono: &[i16]) {
        if self.stream.is_none() {
            return;
        }
        push_chunk(&mut lock_queue(&self.queue), mono);
    }

    fn shutdown(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.pause();
        }
        lock_queue(&self.queue).clear();
    }
}

// ---------------------------------------------------------------------------
// Window + input: a winit `ApplicationHandler` driven by `pump_app_events`,
// which is what lets a winit window live inside `main.rs`'s own
// poll/update/render loop instead of winit owning the loop.
// ---------------------------------------------------------------------------

struct App {
    title: String,
    /// Logical size the window is created at (the X1 tier for every current
    /// caller, matching `win32::Platform::init`).
    initial_size: LogicalSize<u32>,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    /// Down/up state indexed by Windows VK code, exactly like `win32`'s.
    keys: [bool; 256],
    quit: bool,
}

impl App {
    fn new(title: &str, client_w: i32, client_h: i32) -> App {
        App {
            title: title.to_string(),
            initial_size: LogicalSize::new(client_w.max(1) as u32, client_h.max(1) as u32),
            window: None,
            surface: None,
            keys: [false; 256],
            quit: false,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(self.title.clone())
            .with_inner_size(self.initial_size)
            .with_resizable(true);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                eprintln!("[macos] could not create window: {e}");
                self.quit = true;
                return;
            }
        };
        // softbuffer wants both handles; `Rc<Window>` satisfies both traits,
        // and the game is single-threaded so `Rc` is the right pointer here.
        match softbuffer::Context::new(Rc::clone(&window))
            .and_then(|ctx| softbuffer::Surface::new(&ctx, Rc::clone(&window)))
        {
            Ok(surface) => self.surface = Some(surface),
            Err(e) => {
                // A window with no drawable surface would render a frozen
                // grey rectangle and look like a hang; fail loudly instead.
                eprintln!("[macos] could not create draw surface: {e}");
                self.quit = true;
            }
        }
        self.window = Some(window);
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => self.quit = true,
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if let Some(vk) = vk_for(code) {
                        self.keys[vk as usize] = event.state == ElementState::Pressed;
                    }
                }
            }
            WindowEvent::Focused(false) => {
                // Dropping focus mid-press would otherwise leave the key stuck
                // down forever, since the KeyUp goes to whoever took focus.
                self.keys = [false; 256];
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Public safe API — identical surface to `win32::Platform`.
// ---------------------------------------------------------------------------

/// Owns the winit event loop and window, its softbuffer draw surface, key
/// state, timing, and the cpal playback ring.
pub struct Platform {
    event_loop: EventLoop<()>,
    app: App,
    start: Instant,
    audio: AudioRing,
    /// Last-applied window-scale mode, so re-applying the same mode every time
    /// the settings screen is exited is a cheap no-op.
    window_scale: WindowScaleMode,
}

impl Platform {
    /// Creates a window whose CLIENT area is `client_w`x`client_h` logical
    /// points and pumps the event loop until it exists (winit only creates
    /// windows from inside `resumed`, but this constructor must hand back a
    /// live window like `win32`'s does).
    pub fn init(title: &str, client_w: i32, client_h: i32) -> Platform {
        let mut event_loop = EventLoop::new().expect("could not create the macOS event loop");
        event_loop.set_control_flow(ControlFlow::Poll);
        let mut app = App::new(title, client_w, client_h);

        // `resumed` fires on the first pump in practice; the bound is only a
        // guard against spinning forever if it never does.
        for _ in 0..600 {
            if let PumpStatus::Exit(_) = event_loop.pump_app_events(Some(Duration::ZERO), &mut app)
            {
                app.quit = true;
            }
            if app.window.is_some() || app.quit {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if app.window.is_none() {
            eprintln!("[macos] window never became available");
            app.quit = true;
        }

        Platform {
            event_loop,
            app,
            start: Instant::now(),
            audio: AudioRing::silent(),
            // `init` always creates the window at exactly `client_w`x`client_h`,
            // which is the X1 case for every current caller, so a boot-time
            // `set_window_scale(X1)` is correctly seen as a no-op.
            window_scale: WindowScaleMode::X1,
        }
    }

    /// Opens the cpal output stream. Call once after `init`. Safe to call with
    /// no audio device present — the ring degrades to a silent no-op path and
    /// the game still runs.
    pub fn audio_init(&mut self) {
        self.audio = AudioRing::open();
    }

    /// Ring slots currently free to accept a fresh [`AUDIO_BUFFER_FRAMES`]
    /// -long mono chunk (0 if `audio_init` was never called, or no device).
    pub fn audio_free_buffers(&mut self) -> usize {
        self.audio.free_count()
    }

    /// Submit one [`AUDIO_BUFFER_FRAMES`]-length mono chunk, duplicated across
    /// the device's channels by the stream callback (SPEC §8). No-op if there
    /// is no free ring space or no audio device.
    pub fn audio_submit(&mut self, mono: &[i16]) {
        self.audio.submit(mono);
    }

    /// Drains pending window/keyboard events, updating key state. Returns
    /// `false` once the window has been closed (the caller should stop calling
    /// poll and exit).
    pub fn poll(&mut self) -> bool {
        if let PumpStatus::Exit(_) = self
            .event_loop
            .pump_app_events(Some(Duration::ZERO), &mut self.app)
        {
            self.app.quit = true;
        }
        !self.app.quit
    }

    /// Current down/up state of virtual-key `vk`.
    pub fn key(&self, vk: u8) -> bool {
        self.app.keys[vk as usize]
    }

    /// Scales the 0x00RRGGBB `fb` (top-down, `fb_w`x`fb_h`) onto the window's
    /// current client area and presents it. Nearest-neighbour, stretched to
    /// the full client rect — the same "no letterbox" behaviour as `win32`'s
    /// `StretchDIBits`/`COLORONCOLOR` blit. softbuffer's buffer is `0RGB`
    /// per u32, which is byte-for-byte the framebuffer's own format, so no
    /// channel conversion happens here.
    pub fn blit(&mut self, fb: &[u32], fb_w: i32, fb_h: i32) {
        assert_eq!(fb.len(), (fb_w * fb_h) as usize);
        let (Some(window), Some(surface)) = (self.app.window.as_ref(), self.app.surface.as_mut())
        else {
            return;
        };
        let size = window.inner_size();
        let (Some(dst_w), Some(dst_h)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return; // minimized / zero-sized: nothing to present
        };
        if surface.resize(dst_w, dst_h).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else {
            return;
        };

        let (src_w, src_h) = (fb_w as usize, fb_h as usize);
        let (dst_w, dst_h) = (dst_w.get() as usize, dst_h.get() as usize);
        for y in 0..dst_h {
            let src_row = (y * src_h / dst_h) * src_w;
            let dst_row = y * dst_w;
            for x in 0..dst_w {
                buf[dst_row + x] = fb[src_row + (x * src_w / dst_w)];
            }
        }
        let _ = buf.present();
    }

    /// Applies a window-scale tier: the three windowed tiers resize the client
    /// area, `Fullscreen` goes borderless on the current monitor.
    pub fn set_window_scale(&mut self, mode: WindowScaleMode) {
        if mode == self.window_scale {
            return;
        }
        if let Some(window) = self.app.window.as_ref() {
            match mode {
                WindowScaleMode::Fullscreen => {
                    window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                }
                _ => {
                    window.set_fullscreen(None);
                    let (w, h) = client_size_for(mode);
                    // The returned "actual size" is ignored: macOS may clamp a
                    // 2x window to the screen, and `blit` reads the real inner
                    // size every frame anyway.
                    let _ = window.request_inner_size(LogicalSize::new(w, h));
                }
            }
        }
        self.window_scale = mode;
    }

    /// Seconds since an arbitrary epoch, from a monotonic `Instant`.
    pub fn now_s(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// Coarse OS sleep. Pair with a spin-wait for the last sub-millisecond
    /// stretch when pacing frames.
    pub fn sleep_ms(ms: u32) {
        std::thread::sleep(Duration::from_millis(ms as u64));
    }
}

impl Drop for Platform {
    fn drop(&mut self) {
        // Stop pulling from the queue before the window goes away, mirroring
        // `win32`'s "tear down audio first" ordering.
        self.audio.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Save I/O (SPEC §9). The macOS analogue of `%APPDATA%\floppy_spin\save.bin`
// is `~/Library/Application Support/floppy_spin/save.bin`. Both directions are
// best-effort in exactly the same way as `win32`'s: any failure degrades to
// "no save" rather than ever panicking (SPEC §9: "never crash").
// ---------------------------------------------------------------------------

const SAVE_DIR_SUBPATH: &str = "Library/Application Support/floppy_spin";
const SAVE_FILE_NAME: &str = "save.bin";

/// `~/Library/Application Support/floppy_spin`, or `None` if `$HOME` is unset.
fn save_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(SAVE_DIR_SUBPATH))
}

/// Read the whole save file. Returns an empty `Vec` on ANY failure — as on
/// Windows, `save::decode` turns an empty (or otherwise invalid) blob into
/// `SaveState::default()`, so this never distinguishes "missing" from
/// "corrupt".
pub fn save_load() -> Vec<u8> {
    let Some(dir) = save_dir() else {
        return Vec::new();
    };
    std::fs::read(dir.join(SAVE_FILE_NAME)).unwrap_or_default()
}

/// Best-effort save write: create the directory if missing, then write. Never
/// panics, never propagates an error — a failed save is silently dropped.
pub fn save_store(bytes: &[u8]) {
    let Some(dir) = save_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(SAVE_FILE_NAME), bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The VK codes this backend produces are the ones `main.rs` polls. If
    /// these ever drift, the key silently stops working on macOS only, which
    /// is exactly the kind of bug a shared `main.rs` is supposed to prevent.
    #[test]
    fn vk_table_matches_mains_virtual_key_codes() {
        assert_eq!(vk_for(KeyCode::ArrowLeft), Some(0x25));
        assert_eq!(vk_for(KeyCode::ArrowUp), Some(0x26));
        assert_eq!(vk_for(KeyCode::ArrowRight), Some(0x27));
        assert_eq!(vk_for(KeyCode::ArrowDown), Some(0x28));
        assert_eq!(vk_for(KeyCode::Space), Some(0x20));
        assert_eq!(vk_for(KeyCode::ShiftLeft), Some(0x10));
        assert_eq!(vk_for(KeyCode::ShiftRight), Some(0x10));
        assert_eq!(vk_for(KeyCode::ControlLeft), Some(0x11));
        assert_eq!(vk_for(KeyCode::ControlRight), Some(0x11));
        assert_eq!(vk_for(KeyCode::KeyZ), Some(0x5A));
        assert_eq!(vk_for(KeyCode::KeyX), Some(0x58));
        assert_eq!(vk_for(KeyCode::KeyC), Some(0x43));
        assert_eq!(vk_for(KeyCode::Escape), Some(VK_ESCAPE));
    }

    /// Keys the game does not use must not land in the VK array at all.
    #[test]
    fn unmapped_keys_are_ignored() {
        assert_eq!(vk_for(KeyCode::KeyQ), None);
        assert_eq!(vk_for(KeyCode::F1), None);
        assert_eq!(vk_for(KeyCode::Tab), None);
    }

    /// Scale tiers are exact multiples of the 960x540 internal resolution;
    /// fullscreen keeps the base size since the monitor decides the rest.
    #[test]
    fn scale_tiers_are_exact_multiples_of_the_base_resolution() {
        assert_eq!(client_size_for(WindowScaleMode::X1), (960, 540));
        assert_eq!(client_size_for(WindowScaleMode::X1_5), (1440, 810));
        assert_eq!(client_size_for(WindowScaleMode::X2), (1920, 1080));
        assert_eq!(client_size_for(WindowScaleMode::Fullscreen), (960, 540));
    }

    /// A silent ring (no device) reports zero free chunks, which is what makes
    /// `main.rs`'s submit loop a no-op instead of spinning on a queue nothing
    /// ever drains.
    #[test]
    fn silent_ring_accepts_nothing() {
        let mut ring = AudioRing::silent();
        assert_eq!(ring.free_count(), 0);
        ring.submit(&[0i16; AUDIO_BUFFER_FRAMES]);
        assert_eq!(lock_queue(&ring.queue).len(), 0);
    }

    /// Chunk accounting matches `win32`'s ring depth: four chunks in flight,
    /// counted whole.
    #[test]
    fn ring_counts_whole_chunks() {
        assert_eq!(
            AUDIO_RING_CAPACITY,
            AUDIO_RING_BUFFERS * AUDIO_BUFFER_FRAMES
        );
        assert_eq!(free_chunks(0), 4);
        assert_eq!(free_chunks(AUDIO_BUFFER_FRAMES), 3);
        assert_eq!(free_chunks(AUDIO_RING_CAPACITY), 0);
        // A partially-drained chunk does not count as room for a whole one.
        assert_eq!(free_chunks(AUDIO_RING_CAPACITY - 1), 0);
    }

    /// A submit that would overflow is dropped whole, never truncated — the
    /// queue must only ever hold complete chunks.
    #[test]
    fn overflowing_chunk_is_dropped_not_truncated() {
        let mut q: VecDeque<i16> = VecDeque::new();
        let chunk = [1i16; AUDIO_BUFFER_FRAMES];
        for i in 1..=AUDIO_RING_BUFFERS {
            assert!(push_chunk(&mut q, &chunk), "chunk {i} should fit");
        }
        assert_eq!(q.len(), AUDIO_RING_CAPACITY);
        assert!(!push_chunk(&mut q, &chunk), "the fifth chunk must not fit");
        assert_eq!(q.len(), AUDIO_RING_CAPACITY, "nothing partial was written");
    }

    /// The save path is the macOS convention, not a `%APPDATA%` transliteration.
    #[test]
    fn save_dir_is_under_application_support() {
        if let Some(dir) = save_dir() {
            assert!(dir.ends_with("Library/Application Support/floppy_spin"));
            assert!(dir.is_absolute());
        }
    }
}
