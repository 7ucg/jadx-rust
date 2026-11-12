//! Bounded posting store: fixed size records sorted by a 64-bit fingerprint, kept in memory or
//! spilled to sorted runs on disk and merged into one file that lookups binary-search.
//!
//! Adapted from RDX (Apache-2.0, Ch0pin and RDX contributors, `src/invoke_index.rs`): the idea
//! of project owned disk postings sorted by fingerprint with a lookup that binary-searches the
//! run instead of scanning it. Modifications: one store for all site kinds, a memory budget with
//! external merge instead of fixed shards, a dependency free fingerprint, no hit verification
//! here (the caller verifies hits against the code).

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::error::{IndexError, Result};

/// Size of one serialised record.
pub const RECORD_BYTES: usize = 20;
/// Bits of the packed word that hold the instruction offset (the rest is the site kind).
pub const OFFSET_BITS: u32 = 27;
pub const MAX_OFFSET: u32 = (1 << OFFSET_BITS) - 1;

/// 64-bit fingerprint of `text` in key space `group` (FNV-1a with a final avalanche).
pub fn fingerprint(group: u8, text: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ u64::from(group).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    for &b in text {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

/// One occurrence of a keyed entity inside a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Record {
    pub key: u64,
    /// Class index in the root.
    pub class: u32,
    /// Method index inside the class.
    pub method: u32,
    /// Site kind in the high bits, instruction offset (code units) in the low [`OFFSET_BITS`].
    pub packed: u32,
}

impl Record {
    pub fn new(key: u64, class: u32, method: u32, kind: u8, offset: u32) -> Record {
        Record {
            key,
            class,
            method,
            packed: (u32::from(kind) << OFFSET_BITS) | (offset & MAX_OFFSET),
        }
    }

    pub fn kind(&self) -> u8 {
        (self.packed >> OFFSET_BITS) as u8
    }

    pub fn offset(&self) -> u32 {
        self.packed & MAX_OFFSET
    }

    fn encode(&self) -> [u8; RECORD_BYTES] {
        let mut b = [0u8; RECORD_BYTES];
        b[..8].copy_from_slice(&self.key.to_le_bytes());
        b[8..12].copy_from_slice(&self.class.to_le_bytes());
        b[12..16].copy_from_slice(&self.method.to_le_bytes());
        b[16..].copy_from_slice(&self.packed.to_le_bytes());
        b
    }

    fn decode(b: &[u8; RECORD_BYTES]) -> Record {
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Record {
            key: u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
            class: u32_at(8),
            method: u32_at(12),
            packed: u32_at(16),
        }
    }
}

/// Bounds of a store.
#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// Bytes of records buffered in memory. Exceeding it spills a sorted run to disk when
    /// `spill_dir` is set and fails with [`IndexError::Limit`] otherwise.
    pub memory_budget: usize,
    /// Parent directory of the private spill directory (created with mode 0700, removed when the
    /// store is dropped). `None` keeps everything in memory.
    pub spill_dir: Option<PathBuf>,
    /// Maximum number of records of the whole store.
    pub max_records: u64,
}

impl Default for StoreConfig {
    fn default() -> Self {
        StoreConfig {
            memory_budget: 256 * 1024 * 1024,
            spill_dir: None,
            max_records: 400_000_000,
        }
    }
}

impl StoreConfig {
    /// Memory only, bounded by `memory_budget` bytes.
    pub fn memory(memory_budget: usize) -> Self {
        StoreConfig {
            memory_budget,
            ..StoreConfig::default()
        }
    }

    /// Buffer `memory_budget` bytes, spill to a private directory below `dir` beyond that.
    pub fn disk(dir: impl Into<PathBuf>, memory_budget: usize) -> Self {
        StoreConfig {
            memory_budget,
            spill_dir: Some(dir.into()),
            ..StoreConfig::default()
        }
    }
}

/// Private directory removed on drop.
#[derive(Debug)]
struct SpillDir(PathBuf);

impl SpillDir {
    fn create(parent: &Path) -> Result<SpillDir> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for _ in 0..8 {
            let path = parent.join(format!(
                "jadx-index-{}-{}-{:x}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed),
                nanos
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(SpillDir(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(IndexError::Invalid(
            "could not create a private spill directory".into(),
        ))
    }
}

impl Drop for SpillDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn create_private(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).read(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

/// Collects records, then sorts them into a [`PostingStore`].
pub struct PostingStoreBuilder {
    config: StoreConfig,
    buffer: Vec<Record>,
    buffer_limit: usize,
    runs: Vec<PathBuf>,
    dir: Option<SpillDir>,
    total: u64,
}

impl PostingStoreBuilder {
    pub fn new(config: StoreConfig) -> PostingStoreBuilder {
        let buffer_limit = (config.memory_budget / std::mem::size_of::<Record>()).max(1);
        PostingStoreBuilder {
            config,
            buffer: Vec::new(),
            buffer_limit,
            runs: Vec::new(),
            dir: None,
            total: 0,
        }
    }

    pub fn len(&self) -> u64 {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    pub fn push(&mut self, record: Record) -> Result<()> {
        if self.total >= self.config.max_records {
            return Err(IndexError::Limit(format!(
                "index exceeds {} records",
                self.config.max_records
            )));
        }
        if self.buffer.len() >= self.buffer_limit {
            self.spill()?;
        }
        self.buffer.push(record);
        self.total += 1;
        Ok(())
    }

    fn spill(&mut self) -> Result<()> {
        let Some(parent) = self.config.spill_dir.clone() else {
            return Err(IndexError::Limit(format!(
                "index exceeds the memory budget of {} bytes",
                self.config.memory_budget
            )));
        };
        if self.dir.is_none() {
            self.dir = Some(SpillDir::create(&parent)?);
        }
        let dir = &self.dir.as_ref().expect("spill dir").0;
        self.buffer.sort_unstable();
        let path = dir.join(format!("run{}.bin", self.runs.len()));
        let mut out = BufWriter::new(create_private(&path)?);
        for r in &self.buffer {
            out.write_all(&r.encode())?;
        }
        out.flush()?;
        self.runs.push(path);
        self.buffer.clear();
        Ok(())
    }

    pub fn finish(mut self) -> Result<PostingStore> {
        if self.runs.is_empty() {
            self.buffer.sort_unstable();
            self.buffer.shrink_to_fit();
            return Ok(PostingStore {
                storage: Storage::Memory(self.buffer),
                _dir: None,
            });
        }
        if !self.buffer.is_empty() {
            self.spill()?;
        }
        let dir = self.dir.take().expect("spill dir");
        let merged_path = dir.0.join("postings.bin");
        let mut out = BufWriter::new(create_private(&merged_path)?);
        let mut readers: Vec<BufReader<File>> = Vec::new();
        for path in &self.runs {
            readers.push(BufReader::with_capacity(1 << 16, File::open(path)?));
        }
        let mut heap = BinaryHeap::new();
        for (i, r) in readers.iter_mut().enumerate() {
            if let Some(rec) = read_record(r)? {
                heap.push(Reverse((rec, i)));
            }
        }
        let mut count = 0u64;
        while let Some(Reverse((rec, i))) = heap.pop() {
            out.write_all(&rec.encode())?;
            count += 1;
            if let Some(next) = read_record(&mut readers[i])? {
                heap.push(Reverse((next, i)));
            }
        }
        out.flush()?;
        drop(out);
        drop(readers);
        for path in &self.runs {
            let _ = fs::remove_file(path);
        }
        let file = File::open(&merged_path)?;
        Ok(PostingStore {
            storage: Storage::Disk {
                file: Mutex::new(file),
                count,
            },
            _dir: Some(dir),
        })
    }
}

fn read_record(r: &mut impl Read) -> Result<Option<Record>> {
    let mut b = [0u8; RECORD_BYTES];
    match r.read_exact(&mut b) {
        Ok(()) => Ok(Some(Record::decode(&b))),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
        Err(e) => Err(e.into()),
    }
}

enum Storage {
    Memory(Vec<Record>),
    Disk { file: Mutex<File>, count: u64 },
}

/// Immutable posting store; lookups by fingerprint.
pub struct PostingStore {
    storage: Storage,
    _dir: Option<SpillDir>,
}

impl std::fmt::Debug for PostingStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostingStore")
            .field("records", &self.len())
            .field("on_disk", &self.is_on_disk())
            .finish()
    }
}

impl PostingStore {
    pub fn len(&self) -> u64 {
        match &self.storage {
            Storage::Memory(v) => v.len() as u64,
            Storage::Disk { count, .. } => *count,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_on_disk(&self) -> bool {
        matches!(self.storage, Storage::Disk { .. })
    }

    /// Records with `key`, at most `limit`, in (class, method, offset) order.
    pub fn lookup(&self, key: u64, limit: usize) -> Result<Vec<Record>> {
        match &self.storage {
            Storage::Memory(v) => {
                let start = v.partition_point(|r| r.key < key);
                Ok(v[start..]
                    .iter()
                    .take_while(|r| r.key == key)
                    .take(limit)
                    .copied()
                    .collect())
            }
            Storage::Disk { file, count } => {
                let mut f = file
                    .lock()
                    .map_err(|_| IndexError::Invalid("posting file lock poisoned".into()))?;
                let mut lo = 0u64;
                let mut hi = *count;
                let mut buf = [0u8; RECORD_BYTES];
                while lo < hi {
                    let mid = lo + (hi - lo) / 2;
                    f.seek(SeekFrom::Start(mid * RECORD_BYTES as u64))?;
                    f.read_exact(&mut buf)?;
                    if Record::decode(&buf).key < key {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                f.seek(SeekFrom::Start(lo * RECORD_BYTES as u64))?;
                let mut reader = BufReader::new(&mut *f);
                let mut out = Vec::new();
                while out.len() < limit {
                    match read_record(&mut reader)? {
                        Some(r) if r.key == key => out.push(r),
                        _ => break,
                    }
                }
                Ok(out)
            }
        }
    }

    /// Number of records with `key` (stops counting at `limit`).
    pub fn count(&self, key: u64, limit: usize) -> Result<usize> {
        Ok(self.lookup(key, limit)?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recs() -> Vec<Record> {
        let mut v = Vec::new();
        for i in 0..1000u32 {
            v.push(Record::new(
                u64::from(i % 37),
                i,
                i % 5,
                (i % 7) as u8,
                i * 3,
            ));
        }
        v
    }

    fn check(store: &PostingStore) {
        assert_eq!(store.len(), 1000);
        for key in 0..37u64 {
            let got = store.lookup(key, usize::MAX).unwrap();
            let want: Vec<_> = {
                let mut w: Vec<_> = recs().into_iter().filter(|r| r.key == key).collect();
                w.sort();
                w
            };
            assert_eq!(got, want, "key {key}");
        }
        assert!(store.lookup(99, 10).unwrap().is_empty());
        assert_eq!(store.lookup(3, 2).unwrap().len(), 2);
    }

    #[test]
    fn memory_store_roundtrip() {
        let mut b = PostingStoreBuilder::new(StoreConfig::memory(1 << 20));
        for r in recs() {
            b.push(r).unwrap();
        }
        let s = b.finish().unwrap();
        assert!(!s.is_on_disk());
        check(&s);
    }

    #[test]
    fn disk_store_merges_runs() {
        // budget of ~100 records forces ten runs
        let mut b = PostingStoreBuilder::new(StoreConfig::disk(std::env::temp_dir(), 2000));
        for r in recs() {
            b.push(r).unwrap();
        }
        let s = b.finish().unwrap();
        assert!(s.is_on_disk());
        let dir = match &s._dir {
            Some(d) => d.0.clone(),
            None => unreachable!(),
        };
        assert!(dir.exists());
        check(&s);
        drop(s);
        assert!(!dir.exists(), "spill directory removed on drop");
    }

    #[test]
    fn memory_budget_and_record_limit_are_enforced() {
        let mut b = PostingStoreBuilder::new(StoreConfig::memory(100));
        let mut failed = false;
        for r in recs() {
            if b.push(r).is_err() {
                failed = true;
                break;
            }
        }
        assert!(failed);
        let mut b = PostingStoreBuilder::new(StoreConfig {
            max_records: 3,
            ..StoreConfig::memory(1 << 20)
        });
        for i in 0..3 {
            b.push(Record::new(1, i, 0, 0, 0)).unwrap();
        }
        assert!(matches!(
            b.push(Record::new(1, 9, 0, 0, 0)),
            Err(IndexError::Limit(_))
        ));
    }

    #[test]
    fn record_packing_and_fingerprints() {
        let r = Record::new(7, 1, 2, 17, 123_456);
        assert_eq!((r.kind(), r.offset()), (17, 123_456));
        assert_eq!(Record::decode(&r.encode()), r);
        assert_ne!(fingerprint(1, b"a"), fingerprint(2, b"a"));
        assert_ne!(fingerprint(1, b"a"), fingerprint(1, b"b"));
        assert_eq!(fingerprint(1, b"abc"), fingerprint(1, b"abc"));
    }
}
