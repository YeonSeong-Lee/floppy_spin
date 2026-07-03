// Game binary. Console subsystem in debug builds only (println! debugging).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod platform;

use platform::win32::{Platform, VK_ESCAPE};

const W: usize = 960;
const H: usize = 540;
const FRAME_DT: f64 = 1.0 / 60.0;
/// Spin-wait the tail of the frame instead of sleeping through it, so pacing
/// lands on the 16.666ms boundary precisely; Sleep()'s ~1ms granularity can't.
const SPIN_MARGIN_S: f64 = 0.0015;

/// Animated test-gradient pixel. Integer math only (SPEC §5 determinism
/// policy: no f32 sin/cos). A sibling headless bin must reproduce this exact
/// formula so their outputs match frame-for-frame.
fn test_pixel(x: usize, y: usize, t: u32) -> u32 {
    let r = ((x * 255) / 959) as u32;
    let g = ((y * 255) / 539) as u32;
    let b = ((x ^ (y + t as usize)) & 0xFF) as u32;
    (r << 16) | (g << 8) | b
}

fn main() {
    let mut platform = Platform::init("FLOPPY SPIN", W as i32, H as i32);
    let mut fb = vec![0u32; W * H];
    let mut frame: u32 = 0;

    loop {
        let frame_start = platform.now_s();

        if !platform.poll() {
            break;
        }
        if platform.key(VK_ESCAPE) {
            break;
        }

        for y in 0..H {
            for x in 0..W {
                fb[y * W + x] = test_pixel(x, y, frame);
            }
        }
        platform.blit(&fb, W as i32, H as i32);
        frame = frame.wrapping_add(1);

        // Pace to 60 fps: sleep off the coarse remainder, leaving a small
        // margin, then spin-wait onto the exact frame boundary. Never
        // busy-spins the whole frame.
        let target = frame_start + FRAME_DT;
        let remaining = target - platform.now_s();
        if remaining > SPIN_MARGIN_S {
            Platform::sleep_ms(((remaining - SPIN_MARGIN_S) * 1000.0) as u32);
        }
        while platform.now_s() < target {}
    }
}
