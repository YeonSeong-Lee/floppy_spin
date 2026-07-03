//! Source-scan guard for SPEC §5 / C5: greps every `.rs` file this workspace
//! directly controls under `core`/`render`/`audio` for libm-transcendental
//! calls and a few other float methods that are not guaranteed bit-identical
//! across platforms/CRTs (e.g. `mul_add` depends on whether the target has
//! hardware FMA). `floppy_core::fixmath` is the one place allowed to
//! implement these from scratch, so its file is exempt from the scan.
//!
//! Both call syntaxes are banned (M1 verifier finding: method-call-only
//! matching left `f32::sin(x)` UFCS spelling invisible):
//!   - method form   `.sin(`
//!   - qualified form `::sin(` — allowed only when the path segment right
//!     before `::` is literally `fixmath` and the function is one fixmath
//!     exports. Consequence: never alias the module (`use fixmath as fm`) —
//!     the scanner would flag `fm::sin(` as a violation, which is the safe
//!     direction to fail.
//!
//! `.floor()`/`.ceil()`/`.abs()`/`.min()`/`.max()`/`.clamp()`/`.trunc()` are
//! IEEE-754-exact (bit-identical on every conforming platform) and are
//! therefore deliberately NOT in the banned list.

use std::fs;
use std::path::{Path, PathBuf};

/// Base names banned outside `fixmath.rs`, in either `.name(` or `::name(`
/// spelling.
const BANNED: &[&str] = &[
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "atan2",
    "sinh",
    "cosh",
    "tanh",
    "sqrt",
    "rsqrt",
    "powf",
    "powi",
    "exp",
    "exp2",
    "ln",
    "log2",
    "log10",
    "hypot",
    "to_radians",
    "to_degrees",
    "rem_euclid",
    "mul_add",
    "round",
    "fract",
];

/// The sanctioned deterministic implementations: `fixmath::<name>(` is the
/// one qualified spelling the scan permits.
const FIXMATH_FNS: &[&str] = &["sin", "cos", "sqrt", "rsqrt", "atan2"];

/// Escape hatch for false positives — e.g. a method named `.round(` on some
/// non-float type that happens to match a banned substring. Any line
/// containing this exact marker is skipped ENTIRELY, so never share a line
/// between an escape-hatched call and other float math. Use sparingly; every
/// real float-precision escape hatch belongs in `fixmath.rs`, not here.
const ALLOW_MARKER: &str = "// libm-ok";

/// File name excluded from the scan entirely: it IS the authoritative
/// libm-replacement implementation (SPEC §5) and is expected to use these
/// operations internally (in f64: LUT construction and sin/cos range
/// reduction, both restricted to IEEE-exact operations) and to define the
/// public `sin`/`cos`/`sqrt`/`atan2`/... functions everything else must call
/// instead.
const EXEMPT_FILE: &str = "fixmath.rs";

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
}

/// The identifier (ASCII alphanumeric/underscore run) that `s` ends with.
fn trailing_ident(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        let c = bytes[i - 1];
        if c.is_ascii_alphanumeric() || c == b'_' {
            i -= 1;
        } else {
            break;
        }
    }
    &s[i..]
}

/// Scan one line for banned calls; returns the banned pattern hit, if any.
/// The comment tail (from the first `//`) is stripped first: doc comments
/// legitimately DISCUSS banned calls (e.g. "not `.round()`, which is
/// banned"), and flagging prose would train people to write worse docs.
/// Known accepted blind spot: a `//` inside a string literal truncates the
/// scan of that line's remainder.
fn line_violation(line: &str) -> Option<String> {
    let line = match line.find("//") {
        Some(pos) => &line[..pos],
        None => line,
    };
    for name in BANNED {
        let method_form = format!(".{name}(");
        if line.contains(&method_form) {
            return Some(method_form);
        }
        let qualified_form = format!("::{name}(");
        for (pos, _) in line.match_indices(&qualified_form) {
            let sanctioned =
                FIXMATH_FNS.contains(name) && trailing_ident(&line[..pos]) == "fixmath";
            if !sanctioned {
                return Some(qualified_form);
            }
        }
    }
    None
}

#[test]
fn no_libm_calls_outside_fixmath() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.join("..").join("..");

    let scan_dirs = [
        workspace_root
            .join("crates")
            .join("floppy_core")
            .join("src"),
        workspace_root
            .join("crates")
            .join("floppy_render")
            .join("src"),
        workspace_root
            .join("crates")
            .join("floppy_audio")
            .join("src"),
    ];

    let mut files = Vec::new();
    for dir in &scan_dirs {
        collect_rs_files(dir, &mut files);
    }
    assert!(
        !files.is_empty(),
        "no-libm scan found zero .rs files under {:?} — path resolution is broken",
        scan_dirs
    );

    let mut violations = Vec::new();
    for file in &files {
        if file.file_name().and_then(|n| n.to_str()) == Some(EXEMPT_FILE) {
            continue;
        }
        let contents = fs::read_to_string(file).unwrap_or_default();
        for (line_no, line) in contents.lines().enumerate() {
            if line.contains(ALLOW_MARKER) {
                continue;
            }
            if let Some(banned) = line_violation(line) {
                violations.push(format!(
                    "{}:{}: contains {:?}: {}",
                    file.display(),
                    line_no + 1,
                    banned,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "banned libm-equivalent call(s) found outside fixmath.rs:\n{}",
        violations.join("\n")
    );
}

#[test]
fn scanner_catches_both_spellings() {
    // Self-test of the scanner (M1 verifier finding: the previous version
    // silently missed the qualified spelling).
    assert!(line_violation("let a = x.sin();").is_some());
    assert!(line_violation("let a = f32::sin(x);").is_some());
    assert!(line_violation("let a = <f32>::sqrt(x);").is_some());
    assert!(line_violation("let a = f64::mul_add(a, b, c);").is_some());
    assert!(line_violation("let a = fixmath::sin(x);").is_none());
    assert!(line_violation("let a = crate::fixmath::atan2(y, x);").is_none());
    // Aliased module: flagged by design (safe-direction false positive).
    assert!(line_violation("let a = fm::sin(x);").is_some());
    // Comments merely DISCUSSING banned calls are not violations…
    assert!(line_violation("// truncating cast, not `.round()`, which is banned").is_none());
    assert!(line_violation("//! `.round()` and `.fract()` are banned").is_none());
    // …but code before the comment is still scanned.
    assert!(line_violation("let a = x.sin(); // see docs").is_some());
    // fixmath may only vouch for functions it actually exports.
    assert!(line_violation("let a = fixmath::powf(x, y);").is_some());
    assert!(line_violation("let a = x.floor();").is_none());
}
