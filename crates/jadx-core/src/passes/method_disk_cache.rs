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

/// Cap on the bytes currently held on disk (incremented by `store`, decremented by `take`)
/// before `store` starts refusing new entries, falling back to the caller's own "keep in memory
/// instead" path rather than filling the disk without bound. This tracks the live on-disk size,
/// not a cumulative total, so a run that spills and reads a method back many times (the general
/// cross-class dependency case, which round-trips the same state repeatedly) is bounded by its
/// peak working set, not by how often it spills. Generous because the store is on a real disk:
/// the point is to hold the bulk of a very large program's method state off the heap.
const DISK_CACHE_BUDGET_BYTES: u64 = 16_000_000_000;

/// The base directory the spill directory is created under. This cache exists to move decoded
/// method state out of RAM, so the one thing it must not do is write to a RAM-backed filesystem:
/// `std::env::temp_dir()` is `/tmp`, which is frequently `tmpfs` (RAM), and spilling there would
/// free no memory at all. Resolution order:
/// 1. `JADX_RS_METHOD_CACHE_DIR` if set and non-empty (lets a caller pin the location, e.g. a
///    fast local disk, or force `/tmp` back for a RAM disk that is genuinely desired).
/// 2. On Linux, the first of `/var/tmp` then `/tmp` that `/proc/mounts` reports as *not* a
///    `tmpfs`/`ramfs` mount - `/var/tmp` is the FHS-blessed persistent, disk-backed temp dir.
/// 3. `std::env::temp_dir()` as a last-resort fallback (better a bounded RAM spill than none).
fn cache_base() -> PathBuf {
    if let Ok(dir) = std::env::var("JADX_RS_METHOD_CACHE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    #[cfg(target_os = "linux")]
    {
        for cand in ["/var/tmp", "/tmp"] {
            let p = std::path::Path::new(cand);
            if p.is_dir() && is_disk_backed(p) {
                return PathBuf::from(cand);
            }
        }
    }
    std::env::temp_dir()
}

/// Whether `path`'s filesystem is disk-backed (not `tmpfs`/`ramfs`), by matching it against the
/// longest mount point prefix in `/proc/mounts`. Conservative: an unreadable `/proc/mounts` or an
/// unmatched path returns false, so an unknown location is treated as possibly RAM-backed and
/// skipped in favor of the plain `temp_dir()` fallback.
#[cfg(target_os = "linux")]
fn is_disk_backed(path: &std::path::Path) -> bool {
    let Ok(mounts) = fs::read_to_string("/proc/mounts") else {
        return false;
    };
    let target = path.to_string_lossy();
    let mut best_len = 0usize;
    let mut best_fs: &str = "";
    for line in mounts.lines() {
        let mut it = line.split_whitespace();
        let _dev = it.next();
        let (Some(mp), Some(fs)) = (it.next(), it.next()) else {
            continue;
        };
        let matches = target == mp || mp == "/" || target.starts_with(&format!("{mp}/"));
        if matches && mp.len() >= best_len {
            best_len = mp.len();
            best_fs = fs;
        }
    }
    !best_fs.is_empty() && best_fs != "tmpfs" && best_fs != "ramfs"
}

/// A directory-backed overflow store, one file per spilled method, created lazily (so an app
/// that never needs to spill anything never touches the filesystem) and removed on drop.
#[derive(Default)]
pub struct MethodDiskCache {
    dir: Option<PathBuf>,
    bytes_on_disk: u64,
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
            let base = cache_base().join(format!(
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

    /// Whether this cache has ever created its spill directory, i.e. anything was spilled. False
    /// means no method has left memory, so callers can skip disk-aware work entirely (no
    /// filesystem access and no bookkeeping) on the common no-memory-pressure path.
    pub fn in_use(&self) -> bool {
        self.dir.is_some()
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
        if self.bytes_on_disk.saturating_add(bytes.len() as u64) > DISK_CACHE_BUDGET_BYTES {
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
        self.bytes_on_disk += bytes.len() as u64;
        true
    }

    /// Reads and removes `id`'s entry, if it was spilled. Credits the freed bytes back to the
    /// budget so a spill/read/spill cycle stays bounded by the live on-disk size (see
    /// [`DISK_CACHE_BUDGET_BYTES`]).
    pub fn take(&mut self, id: MethodId) -> Option<MethodState> {
        let dir = self.dir.as_ref()?;
        let path = Self::file_path(dir, id);
        let bytes = fs::read(&path).ok()?;
        let _ = fs::remove_file(&path);
        self.bytes_on_disk = self.bytes_on_disk.saturating_sub(bytes.len() as u64);
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
