//! Test-only temp folders. Each is `<temp>/probatio-test-<name>-<pid>-<n>`, so a sweep or a
//! person can tell it from a real run's folder, and it is removed when the test that owns it ends
//! without a panic. A failing test keeps its folder to look at.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct TempDir(PathBuf);

impl TempDir {
    /// A fresh path. Nothing is created: the test makes what it needs under it.
    pub fn new(name: &str) -> TempDir {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("probatio-test-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        TempDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl std::ops::Deref for TempDir {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<std::ffi::OsStr> for TempDir {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.0.as_os_str()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            // The mutant cache this folder's runs made (its kept target directories) goes too.
            crate::slots::remove_dir_robust(&crate::slots::cache_root().join(crate::slots::key_for(&self.0)));
            crate::slots::remove_dir_robust(&self.0);
        }
    }
}
