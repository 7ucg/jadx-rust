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

/// Total bytes `MethodDiskCache` will ever write before `store` starts refusing new entries,
/// falling back to the caller's own "keep in memory instead" path (same tradeoff as the
/// in-memory budget this overflows from) rather than filling the disk without bound on a
/// pathological input with many large, erroring methods.
const DISK_CACHE_BUDGET_BYTES: u64 = 2_000_000_000;

/// A directory-backed overflow store, one file per spilled method, created lazily (so an app
/// that never needs to spill anything never touches the filesystem) and removed on drop.
#[derive(Default)]
pub struct MethodDiskCache {
    dir: Option<PathBuf>,
    bytes_written: u64,
}

impl MethodDiskCache {
    fn file_path(dir: &std::path::Path, id: MethodId) -> PathBuf {
        dir.join(format!("{}_{}.bin", id.0, id.1))
    }

    /// Creates this run's spill directory if it does not exist yet: a name that is unique
    /// ("probably unique" is not relied on - creation is exclusive) to this process and this
    /// `MethodDiskCache` instance, with permissions restricted to the owner on Unix so another
    /// local user on a shared `/tmp` cannot read or race the directory's contents.
    fn ensure_dir(&mut self) -> Option<&std::path::Path> {
        if self.dir.is_none() {
            let base = std::env::temp_dir().join(format!(
                "jadx-rs-method-cache-{}-{:x}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            if Self::create_private_dir(&base).is_err() {
                // best effort: if this fails, `store` below also fails and the caller's own
                // fallback (keep the state in memory / drop it) applies instead.
                return None;
            }
            self.dir = Some(base);
        }
        self.dir.as_deref()
    }

    #[cfg(unix)]
    fn create_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        // `create_new`-style exclusivity: `DirBuilder::create` is `mkdir(2)`, which fails with
        // `AlreadyExists` rather than following a pre-existing directory or symlink planted by
        // another local user at this (process-id- and nanosecond-qualified, so already hard to
        // predict) path.
        std::fs::DirBuilder::new().mode(0o700).create(dir)
    }

    #[cfg(not(unix))]
    fn create_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::DirBuilder::new().create(dir)
    }

    #[cfg(unix)]
    fn create_private_file(path: &std::path::Path) -> std::io::Result<fs::File> {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    }

    #[cfg(not(unix))]
    fn create_private_file(path: &std::path::Path) -> std::io::Result<fs::File> {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
    }

    /// Whether `id` is currently spilled to disk.
    pub fn contains(&self, id: MethodId) -> bool {
        match &self.dir {
            Some(d) => Self::file_path(d, id).exists(),
            None => false,
        }
    }

    /// Writes `state` to disk, returning whether it succeeded. Fails (without writing anything)
    /// once `DISK_CACHE_BUDGET_BYTES` would be exceeded, or on a genuine I/O/serialization
    /// error; either way the caller falls back to keeping the state in memory, never losing it.
    pub fn store(&mut self, id: MethodId, state: &MethodState) -> bool {
        let Ok(bytes) = bincode::serialize(state) else {
            return false;
        };
        if self.bytes_written.saturating_add(bytes.len() as u64) > DISK_CACHE_BUDGET_BYTES {
            return false;
        }
        let Some(dir) = self.ensure_dir() else {
            return false;
        };
        let path = Self::file_path(dir, id);
        use std::io::Write;
        let Ok(mut file) = Self::create_private_file(&path) else {
            return false;
        };
        if file.write_all(&bytes).is_err() {
            let _ = fs::remove_file(&path);
            return false;
        }
        self.bytes_written += bytes.len() as u64;
        true
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
