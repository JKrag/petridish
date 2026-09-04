//! Test-only scratch directories.
//!
//! Mirrors the RAII `Tmp` guard `swab/src/discovery.rs` already uses, rather
//! than adding a `tempfile` dependency: the whole point of ARCHITECTURE.md
//! §8.3 D3 is that this crate's dependency tree stays small enough to read in
//! one sitting, and this is twenty lines.
//!
//! Names are process-unique so a parallel `cargo test` cannot have two binaries
//! fighting over the same directory.

use std::path::PathBuf;

#[derive(Debug)]
pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("petridish_cli_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch dir must be creatable");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
