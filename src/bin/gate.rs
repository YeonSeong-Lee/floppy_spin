//! Ship gate: exe size budget + PE import allowlist (SPEC §12.3).
use std::env;
use std::fs;
use std::path::PathBuf;

const SIZE_LIMIT: u64 = 1_474_560;

const ALLOWED_EXACT: &[&str] = &[
    "kernel32.dll",
    "user32.dll",
    "gdi32.dll",
    "winmm.dll",
    "msvcrt.dll",
    "ntdll.dll",
];
const ALLOWED_PREFIX: &str = "api-ms-win-";

fn default_exe_path() -> Option<PathBuf> {
    let candidates = [
        "target/release/floppy_spin.exe",
        "target/x86_64-pc-windows-gnu/release/floppy_spin.exe",
    ];
    candidates.iter().map(PathBuf::from).find(|p| p.exists())
}

fn fail(reason: &str) -> ! {
    println!("GATE FAIL: {reason}");
    std::process::exit(1);
}

fn main() {
    let mut args = env::args().skip(1);
    let exe_path: PathBuf = match args.next() {
        Some(p) => PathBuf::from(p),
        None => default_exe_path().unwrap_or_else(|| {
            fail(
                "no exe found at target/release/floppy_spin.exe or \
                 target/x86_64-pc-windows-gnu/release/floppy_spin.exe; \
                 run `cargo build --release` first",
            )
        }),
    };

    let bytes = match fs::read(&exe_path) {
        Ok(b) => b,
        Err(e) => fail(&format!("failed to read {}: {e}", exe_path.display())),
    };

    let size = bytes.len() as u64;
    let margin = SIZE_LIMIT as i64 - size as i64;
    println!("size: {size} / {SIZE_LIMIT} bytes (margin {margin})");
    if size > SIZE_LIMIT {
        fail(&format!(
            "exe size {size} exceeds budget {SIZE_LIMIT} bytes"
        ));
    }

    let imports = match floppy_io::pe::imports(&bytes) {
        Ok(names) => names,
        Err(e) => fail(&format!("failed to parse PE imports: {e}")),
    };

    let mut violation = false;
    for name in &imports {
        let lower = name.to_ascii_lowercase();
        let ok = ALLOWED_EXACT.contains(&lower.as_str()) || lower.starts_with(ALLOWED_PREFIX);
        if ok {
            println!("{name}: ok");
        } else {
            println!("{name}: VIOLATION");
            violation = true;
        }
    }

    if violation {
        fail("import outside allowlist");
    }

    println!("GATE PASS");
}
