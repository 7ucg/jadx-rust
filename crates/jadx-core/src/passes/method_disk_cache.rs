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

/// Absolute cap on the bytes currently held on disk (incremented by `store`, decremented by
/// `take`) before `store` starts refusing new entries, falling back to the caller's own "keep in
/// memory instead" path rather than filling the disk without bound. This tracks the live on-disk
/// size, not a cumulative total, so a run that spills and reads a method back many times (the
/// general cross-class dependency case, which round-trips the same state repeatedly) is bounded
/// by its peak working set, not by how often it spills. The effective cap is the smaller of this
/// and a fraction of the spill filesystem's actual free space (see [`store`]), so a small or
/// nearly full `/var/tmp` is never driven to exhaustion - this is only the ceiling for a disk
/// with plenty of room, where the point is to hold the bulk of a very large program's method
/// state off the heap.
const DISK_CACHE_BUDGET_BYTES: u64 = 16_000_000_000;

/// Keep at least this much of the spill filesystem's free space unused, and never let the cache
/// grow past this fraction of the free space it saw when it started, so spilling never fills a
/// user's `/var/tmp` (or wherever `cache_base` resolved to) out from under other programs.
const DISK_FREE_RESERVE_BYTES: u64 = 1_000_000_000;
const DISK_FREE_FRACTION_NUMERATOR: u64 = 1;
const DISK_FREE_FRACTION_DENOMINATOR: u64 = 2;

/// How many bytes `store` may write before re-querying the spill filesystem's free space and
/// refreshing the cap (see [`MethodDiskCache::refresh_cap`]), so a disk filling up mid-run
/// shrinks the cap without paying a `statvfs` on every single spill.
const FREE_RECHECK_INTERVAL_BYTES: u64 = 256_000_000;

/// Bytes currently free on the filesystem that contains `path`, via `statvfs(3)`; `None` if the
/// query fails or on a non-unix target. Used to cap the spill size at a fraction of real free
/// space so the cache cannot exhaust a small or nearly full disk.
#[cfg(unix)]
fn free_disk_bytes(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let mut cpath = path.as_os_str().as_bytes().to_vec();
    cpath.push(0);
    // SAFETY: `cpath` is a NUL-terminated C string that outlives the call, and `stat` is only
    // read after a `0` (success) return.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(cpath.as_ptr() as *const libc::c_char, &mut stat) };
    if rc != 0 {
        return None;
    }
    // `f_bavail` is the blocks available to an unprivileged process; `f_frsize` their size.
    Some((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}

#[cfg(not(unix))]
fn free_disk_bytes(_path: &std::path::Path) -> Option<u64> {
    None
}

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
    /// Effective byte cap, (re)computed by [`MethodDiskCache::refresh_cap`]: the smaller of
    /// [`DISK_CACHE_BUDGET_BYTES`] and the current on-disk size plus a fraction of the spill
    /// filesystem's *current* free space. Zero until the directory is first created, but `store`
    /// always creates it (setting this) before reading it.
    cap_bytes: u64,
    /// `store` re-queries free space and refreshes `cap_bytes` once cumulative writes pass this
    /// mark, so the cap shrinks as the disk fills (from this process or others) instead of
    /// trusting a one-time snapshot.
    next_free_check_at: u64,
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
            self.refresh_cap();
        }
        self.dir.as_deref()
    }

    /// Recomputes [`MethodDiskCache::cap_bytes`] from the spill filesystem's *current* free
    /// space: the smaller of [`DISK_CACHE_BUDGET_BYTES`] and the current on-disk size plus half
    /// the free space beyond a reserve, so the cache never drives a small or filling disk to
    /// exhaustion. Called when the directory is created and again from `store` every
    /// [`FREE_RECHECK_INTERVAL_BYTES`] written, so a disk filling up (from this process or
    /// another) shrinks the cap rather than being trusted from a one-time snapshot. If free
    /// space cannot be determined (e.g. `statvfs` failed, or a non-unix target), only the
    /// absolute budget applies - failing closed to a tiny cap would instead cripple the spill
    /// and reintroduce the very memory collapse this cache exists to prevent, while the absolute
    /// budget plus the full-disk write fallback in `store` already bound the worst case.
    fn refresh_cap(&mut self) {
        let Some(dir) = self.dir.as_deref() else {
            return;
        };
        let free_cap = match free_disk_bytes(dir) {
            Some(free) => self.bytes_on_disk.saturating_add(
                free.saturating_sub(DISK_FREE_RESERVE_BYTES)
                    .saturating_mul(DISK_FREE_FRACTION_NUMERATOR)
                    / DISK_FREE_FRACTION_DENOMINATOR,
            ),
            None => u64::MAX,
        };
        self.cap_bytes = DISK_CACHE_BUDGET_BYTES.min(free_cap);
        self.next_free_check_at = self
            .bytes_on_disk
            .saturating_add(FREE_RECHECK_INTERVAL_BYTES);
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
    /// once the effective cap (see [`MethodDiskCache::cap_bytes`]) would be exceeded, or on a
    /// genuine I/O/serialization error; either way the caller falls back to keeping the state in
    /// memory, never losing it.
    pub fn store(&mut self, id: MethodId, state: &MethodState) -> bool {
        let Ok(bytes) = bincode::serialize(state) else {
            return false;
        };
        let add = bytes.len() as u64;
        // `ensure_dir` creates the directory and sets `cap_bytes` from the spill filesystem's
        // free space the first time, so the cap check below must come after it.
        let path = {
            let Some(dir) = self.ensure_dir() else {
                return false;
            };
            Self::file_path(dir, id)
        };
        // Periodically re-sample free space so the cap tracks a filling disk, not just the
        // snapshot taken when the directory was created.
        if self.bytes_on_disk >= self.next_free_check_at {
            self.refresh_cap();
        }
        if self.bytes_on_disk.saturating_add(add) > self.cap_bytes {
            return false;
        }
        use std::io::Write;
        let Ok(mut file) = Self::create_private_file(&path) else {
            return false;
        };
        if file.write_all(&bytes).is_err() {
            let _ = fs::remove_file(&path);
            return false;
        }
        self.bytes_on_disk += add;
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
