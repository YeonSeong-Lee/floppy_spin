//! Crash-safe save replacement shared by platform adapters.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Write beside the destination, flush the bytes, then atomically replace.
/// The destination is never opened or truncated before the temporary file is
/// durable, so every error before `rename` preserves the previous save.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_write_with_replace(path, bytes, |from, to| fs::rename(from, to))
}

/// Variant for hosts whose native atomic-replace primitive differs from
/// [`fs::rename`] (notably Windows when the destination already exists).
pub fn atomic_write_with_replace(
    path: &Path,
    bytes: &[u8],
    replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "save path has no parent"))?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("save");
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        sequence
    ));

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        replace(&temp, path)?;
        // Directory syncing is supported on Unix. Treat unsupported directory
        // handles as best-effort because the replaced file itself is durable.
        if let Ok(dir) = OpenOptions::new().read(true).open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_file_without_exposing_a_partial_write() {
        let dir =
            std::env::temp_dir().join(format!("floppy-spin-save-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("save.bin");
        fs::write(&path, b"old").unwrap();
        atomic_write(&path, b"complete-new-save").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"complete-new-save");
        assert!(!dir.join("save.bin.tmp").exists());
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(dir);
    }

    #[test]
    fn replace_failure_preserves_the_previous_save_and_cleans_temp() {
        let dir = std::env::temp_dir().join(format!(
            "floppy-spin-save-failure-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("save.bin");
        fs::write(&path, b"known-good").unwrap();
        let result = atomic_write_with_replace(&path, b"new", |_, _| {
            Err(io::Error::other("injected replace failure"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"known-good");
        assert!(!dir.join("save.bin.tmp").exists());
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir(dir);
    }
}
