//! On-disk overflow for decoded method state (`MethodState`) that must outlive its own
//! top-level class's processing - a dependency another, not-yet-processed class still needs, or
//! a method kept alive across classes for `ClassDriver::throws_visit` (see `core_gen.rs`'s
//! `DriverLoader::load` and `ClassDriver::method_has_error`'s doc comments).
//!
//! Keeping every such entry resident in memory is exactly the pattern the reference decompiler
//! itself moves off of for very large inputs (its own "code cache mode" switches from memory to
//! disk) - a dependency graph with many classes still in flight can otherwise hold far more
//! decoded method state alive at once than any single class's own processing needs. This cache
//! is the equivalent for jadx-rs: entries that would otherwise just sit in `ClassDriver::methods`
//! between uses are written here instead and read back on demand.

use std::fs;
use std::path::PathBuf;

use super::driver::MethodState;
use crate::model::node::MethodId;

/// A directory-backed overflow store, one file per spilled method, created lazily (so an app
/// that never needs to spill anything never touches the filesystem) and removed on drop.
#[derive(Default)]
pub struct MethodDiskCache {
    dir: Option<PathBuf>,
}

impl MethodDiskCache {
    fn file_path(dir: &std::path::Path, id: MethodId) -> PathBuf {
        dir.join(format!("{}_{}.bin", id.0, id.1))
    }

    fn ensure_dir(&mut self) -> &std::path::Path {
        if self.dir.is_none() {
            let dir = std::env::temp_dir().join(format!(
                "jadx-rs-method-cache-{}-{:x}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            // best effort: if this fails, `store` below also fails and the caller's own
            // fallback (keep the state in memory / drop it) applies instead.
            let _ = fs::create_dir_all(&dir);
            self.dir = Some(dir);
        }
        self.dir.as_deref().unwrap()
    }

    /// Whether `id` is currently spilled to disk.
    pub fn contains(&self, id: MethodId) -> bool {
        match &self.dir {
            Some(d) => Self::file_path(d, id).exists(),
            None => false,
        }
    }

    /// Writes `state` to disk, returning whether it succeeded (serialization or I/O can fail;
    /// the caller then falls back to keeping the state in memory, never losing it).
    pub fn store(&mut self, id: MethodId, state: &MethodState) -> bool {
        let Ok(bytes) = bincode::serialize(state) else {
            return false;
        };
        let path = Self::file_path(self.ensure_dir(), id);
        fs::write(path, bytes).is_ok()
    }

    /// Reads and removes `id`'s entry, if it was spilled.
    pub fn take(&mut self, id: MethodId) -> Option<MethodState> {
        let dir = self.dir.as_ref()?;
        let path = Self::file_path(dir, id);
        let bytes = fs::read(&path).ok()?;
        let _ = fs::remove_file(&path);
        bincode::deserialize(&bytes).ok()
    }
}

impl Drop for MethodDiskCache {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = fs::remove_dir_all(dir);
        }
    }
}
