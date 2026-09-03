//! Repository guard for the production safety boundary.

use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(root: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root).expect("source directory must be readable") {
        let path = entry.expect("source entry must be readable").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn unsafe_code_is_confined_to_the_win32_ffi_adapter() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let allowed = root.join("src/platform/win32.rs");
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("crates"), &mut files);

    let mut violations = Vec::new();
    for path in files {
        if path == allowed {
            continue;
        }
        let source = fs::read_to_string(&path).expect("Rust source must be UTF-8");
        if source.contains("unsafe {") || source.contains("unsafe fn") {
            violations.push(path.strip_prefix(root).unwrap_or(&path).to_owned());
        }
    }
    assert!(
        violations.is_empty(),
        "unsafe code escaped src/platform/win32.rs: {violations:?}"
    );
}
