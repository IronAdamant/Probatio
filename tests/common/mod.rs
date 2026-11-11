//! Test-only temp folders: `<temp>/probatio-test-<name>-<pid>-<n>`, removed when the owning test
//! ends without a panic. A failing test keeps its folder to look at.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> TempDir {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("probatio-test-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        TempDir(path)
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
        if std::thread::panicking() {
            return;
        }
        // The mutant cache its runs made, in the test cache `probatio_command` points at.
        let key = std::env::temp_dir().join("probatio-test-cache").join(probatio::slots::key_for(&self.0));
        let _ = probatio::slots::remove_dir_robust(&key);
        // A suite process that just exited can hold a handle on Windows for a moment.
        for attempt in 0..6u64 {
            if std::fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50 << attempt));
        }
    }
}
