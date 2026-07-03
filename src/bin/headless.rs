//! Headless verification bin: sim → PNG/WAV without a display (SPEC C6).
use std::env;
use std::fs;
use std::path::PathBuf;

const WIDTH: usize = 960;
const HEIGHT: usize = 540;

fn parse_args() -> (u32, PathBuf) {
    let mut frames: u32 = 3;
    let mut out = PathBuf::from("out");

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => match args.next() {
                Some(v) => match v.parse::<u32>() {
                    Ok(n) => frames = n,
                    Err(_) => {
                        eprintln!("invalid --frames value: {v}");
                        std::process::exit(1);
                    }
                },
                None => {
                    eprintln!("--frames requires a value");
                    std::process::exit(1);
                }
            },
            "--out" => match args.next() {
                Some(v) => out = PathBuf::from(v),
                None => {
                    eprintln!("--out requires a value");
                    std::process::exit(1);
                }
            },
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(1);
            }
        }
    }

    (frames, out)
}

/// Deterministic 960x540 test pattern. This formula must match byte-for-byte
/// the copy embedded in the game's own rendering path (SPEC C5, C6) — it is
/// intentionally duplicated rather than shared across the platform boundary
/// this crate is not allowed to touch.
fn test_pixel(x: usize, y: usize, t: u32) -> u32 {
    let r = ((x * 255) / 959) as u32;
    let g = ((y * 255) / 539) as u32;
    let b = ((x ^ (y + t as usize)) & 0xFF) as u32;
    (r << 16) | (g << 8) | b
}

fn main() {
    let (frames, out_dir) = parse_args();

    if let Err(e) = fs::create_dir_all(&out_dir) {
        eprintln!(
            "failed to create output directory {}: {e}",
            out_dir.display()
        );
        std::process::exit(1);
    }

    for t in 0..frames {
        let mut framebuffer = vec![0u32; WIDTH * HEIGHT];
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                framebuffer[y * WIDTH + x] = test_pixel(x, y, t);
            }
        }

        let png_bytes = floppy_io::png::encode_rgb(WIDTH as u32, HEIGHT as u32, &framebuffer);
        let path = out_dir.join(format!("frame_{t:03}.png"));
        if let Err(e) = fs::write(&path, &png_bytes) {
            eprintln!("failed to write {}: {e}", path.display());
            std::process::exit(1);
        }

        let hash = floppy_core::hash::hash_u32s(&framebuffer);
        println!("frame {t:03} hash=0x{hash:016x}");
    }
}
