//! Bounded, native access to APKs and split-package containers (`.xapk`, `.apks`, `.apkm`).
//!
//! A container exposes the members of every contained APK under logical, split-aware paths:
//!
//! * members of the base APK keep their own path (`classes.dex`, `res/layout/a.xml`),
//! * members of a split APK are prefixed with `splits/<split name>/`,
//! * files of the outer `.xapk` archive that are not APKs (OBB files, icons, `manifest.json`)
//!   are prefixed with `container/`.
//!
//! Two selection policies exist (see [`OpenOptions`]). The default follows the reference jadx
//! plugins (`xapk-input`, `apks-input`, `apkm-input`): every contained APK is part of the input.
//! The strict policy additionally validates the package identity of all splits, picks one
//! bundletool variant of an `.apks` archive and rejects ambiguous containers.
//!
//! Adapted from RDX (Apache-2.0, Ch0pin and RDX contributors, `src/package_container.rs`).
//! Modifications: the reference selection policy, `.apkm` support, `jadx-resources` based
//! manifest decoding, a dependency free JSON reader and reduced archive revalidation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions as FsOpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use crate::error::{ensure, PackageError, Result};
use crate::json::Json;
use crate::manifest::{parse_manifest_meta, SplitMeta};
use crate::toc::parse_toc;

/// Maximum number of APKs inside one container.
pub const MAX_NESTED_APKS: usize = 256;
/// Maximum size of one contained APK.
pub const MAX_NESTED_APK_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Maximum total size of all contained APKs.
pub const MAX_NESTED_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Maximum size of the entry inventory (paths and bookkeeping) of one container.
pub const MAX_CONTAINER_METADATA: usize = 32 * 1024 * 1024;
/// Maximum number of entries of one ZIP archive.
pub const MAX_ZIP_ENTRIES: usize = 100_000;
const MAX_ZIP_DIRECTORY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 4096;

/// What kind of file a path is, by extension and content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    /// A plain APK (or other zip that is read as one APK).
    Apk,
    /// `.xapk`: a zip with `manifest.json` and the APKs of a split package.
    Xapk,
    /// `.apks`: a bundletool archive of APKs.
    Apks,
    /// `.apkm`: an archive with `info.json` and the APKs of a split package.
    Apkm,
}

impl PackageKind {
    /// Kind by file extension (case insensitive); everything else is a plain APK.
    pub fn from_path(path: &Path) -> PackageKind {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("xapk") => PackageKind::Xapk,
            Some("apks") => PackageKind::Apks,
            Some("apkm") => PackageKind::Apkm,
            _ => PackageKind::Apk,
        }
    }

    pub fn is_split_container(self) -> bool {
        self != PackageKind::Apk
    }
}

/// Selection policy of [`PackageContainer::open`].
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenOptions {
    /// Validate the identity of every split (same package and version code, exactly one base,
    /// unique split names, existing feature parents), select one variant of an `.apks` archive
    /// by its `toc.pb` and fail on ambiguous containers. Off: load every contained APK like the
    /// reference jadx plugins do.
    pub strict: bool,
}

/// One file of the package, addressed by its logical path.
#[derive(Debug, Clone)]
pub struct ContainerEntry {
    pub index: usize,
    /// Logical, split-aware path (see the module documentation).
    pub path: String,
    /// Name of the contained APK in the outer archive; empty for plain APKs, `container` for
    /// files of the outer archive itself.
    pub source: String,
    /// Path of the member inside its own archive.
    pub member_path: String,
    pub size: u64,
    pub compressed_size: u64,
    pub crc32: u32,
    /// `false` for files of the outer archive that are not part of any APK.
    pub is_apk_member: bool,
    source_index: usize,
    zip_index: usize,
}

impl ContainerEntry {
    /// Path as the reference jadx names resources of a container: `<apk name>/<member>` for APK
    /// members, the plain member path for outer files.
    pub fn reference_path(&self) -> String {
        if self.is_apk_member && !self.source.is_empty() {
            format!("{}/{}", self.source, self.member_path)
        } else {
            self.member_path.clone()
        }
    }
}

/// Information about one contained APK.
#[derive(Debug, Clone)]
pub struct ApkInfo {
    /// Name in the outer archive (empty for a plain APK).
    pub name: String,
    /// Split name, `None` for the base APK.
    pub split: Option<String>,
    pub meta: Option<SplitMeta>,
    /// Prefix of the logical paths of this APK's members (`""` for the base).
    pub path_prefix: String,
}

#[derive(Debug)]
struct TemporaryApk(PathBuf);

impl Drop for TemporaryApk {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[derive(Debug)]
struct Source {
    path: PathBuf,
    outer_name: String,
    _temporary: Option<TemporaryApk>,
}

/// An opened package: the contained APKs and a flat inventory of all their files.
#[derive(Debug)]
pub struct PackageContainer {
    path: PathBuf,
    kind: PackageKind,
    original_size: u64,
    original_modified: Option<SystemTime>,
    sources: Vec<Source>,
    apks: Vec<ApkInfo>,
    entries: Vec<ContainerEntry>,
    by_path: BTreeMap<String, usize>,
    summary: Option<String>,
    warnings: Vec<String>,
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && name
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn safe_split(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// Bound the ZIP directory before the ZIP library allocates its entry table.
///
/// Adapted from RDX (`check_zip_directory`): ZIP64 archives are accepted (their sizes cannot be
/// checked from the classic end record).
pub fn check_zip_directory(file: &mut File) -> Result<()> {
    let size = file.metadata()?.len();
    let tail_size = size.min(65_557) as usize;
    file.seek(SeekFrom::End(-(tail_size as i64)))?;
    let mut tail = vec![0; tail_size];
    file.read_exact(&mut tail)?;
    let position = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| {
            tail[i..i + 4] == *b"PK\x05\x06"
                && i + 22 + usize::from(u16::from_le_bytes([tail[i + 20], tail[i + 21]]))
                    == tail.len()
        })
        .ok_or_else(|| PackageError::invalid("missing standard ZIP end record"))?;
    let short = |i: usize| u16::from_le_bytes([tail[position + i], tail[position + i + 1]]);
    let word = |i: usize| {
        u32::from_le_bytes([
            tail[position + i],
            tail[position + i + 1],
            tail[position + i + 2],
            tail[position + i + 3],
        ])
    };
    ensure!(
        short(4) == 0 && short(6) == 0 && short(8) == short(10),
        "multi-disk ZIP archives are unsupported"
    );
    file.rewind()?;
    if short(10) == u16::MAX || word(12) == u32::MAX || word(16) == u32::MAX {
        return Ok(()); // ZIP64
    }
    if usize::from(short(10)) > MAX_ZIP_ENTRIES {
        return Err(PackageError::Limit("archive has too many entries".into()));
    }
    if u64::from(word(12)) > MAX_ZIP_DIRECTORY_BYTES {
        return Err(PackageError::Limit(
            "archive central directory is too large".into(),
        ));
    }
    let end_record = size - tail_size as u64 + position as u64;
    ensure!(
        u64::from(word(16)) + u64::from(word(12)) <= end_record,
        "invalid ZIP central directory range"
    );
    Ok(())
}

fn read_zip_entry(zip: &mut zip::ZipArchive<File>, index: usize, limit: u64) -> Result<Vec<u8>> {
    let mut entry = zip.by_index(index)?;
    if entry.size() > limit {
        return Err(PackageError::Limit(format!(
            "archive member exceeds {limit} byte limit"
        )));
    }
    let mut bytes = Vec::new();
    entry.by_ref().take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == entry.size(),
        "archive member size mismatch"
    );
    Ok(bytes)
}

fn temporary_apk() -> Result<(File, TemporaryApk)> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    for _ in 0..8 {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let name = format!(
            "jadx-rs-split-{}-{}-{:x}.apk",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            nanos
        );
        let path = std::env::temp_dir().join(name);
        let mut options = FsOpenOptions::new();
        options.write(true).read(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((file, TemporaryApk(path))),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(PackageError::invalid(
        "could not allocate a temporary split APK",
    ))
}

struct Candidate {
    zip_index: usize,
    name: String,
    size: u64,
}

fn file_stem(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let stem = base.strip_suffix(".apk").unwrap_or(base);
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "split".to_owned()
    } else {
        cleaned
    }
}

impl PackageContainer {
    /// Open `path` (any zip). Plain APKs yield a single source; `.xapk`, `.apks` and `.apkm`
    /// archives are unpacked to temporary files (removed when the container is dropped).
    pub fn open(path: &Path, opts: &OpenOptions) -> Result<Arc<Self>> {
        let path = path.canonicalize()?;
        let file = File::open(&path)?;
        let metadata = file.metadata()?;
        let kind = PackageKind::from_path(&path);
        check_zip_directory(&mut File::open(&path)?)?;
        let mut outer = zip::ZipArchive::new(file)?;
        let mut warnings = Vec::new();
        let mut sources: Vec<Source> = Vec::new();
        let mut summary = None;

        if kind == PackageKind::Apk {
            sources.push(Source {
                path: path.clone(),
                outer_name: String::new(),
                _temporary: None,
            });
        } else {
            let mut candidates = Self::scan_outer(&mut outer, opts, &mut warnings)?;
            Self::select_candidates(
                &mut outer,
                kind,
                opts,
                &mut candidates,
                &mut summary,
                &mut warnings,
            )?;
            let mut total = 0u64;
            for c in &candidates {
                if c.size > MAX_NESTED_APK_BYTES {
                    return Err(PackageError::Limit("nested APK exceeds 2 GiB".into()));
                }
                total = total.saturating_add(c.size);
                if total > MAX_NESTED_TOTAL_BYTES {
                    return Err(PackageError::Limit("nested APK set exceeds 4 GiB".into()));
                }
                let (mut output, temporary) = temporary_apk()?;
                {
                    let mut entry = outer.by_index(c.zip_index)?;
                    let copied = std::io::copy(&mut entry.by_ref().take(c.size + 1), &mut output)?;
                    ensure!(copied == c.size, "nested APK size mismatch: {}", c.name);
                }
                output.flush()?;
                if let Err(e) = check_zip_directory(&mut File::open(&temporary.0)?) {
                    if opts.strict {
                        return Err(PackageError::invalid(format!(
                            "invalid nested APK {}: {e}",
                            c.name
                        )));
                    }
                    warnings.push(format!("skipped invalid nested APK {}: {e}", c.name));
                    continue;
                }
                sources.push(Source {
                    path: temporary.0.clone(),
                    outer_name: c.name.clone(),
                    _temporary: Some(temporary),
                });
            }
            ensure!(
                !sources.is_empty(),
                "package container contains no usable APK members"
            );
        }

        let mut apks = Self::identify(kind, opts, &mut sources, &mut warnings, &mut summary)?;
        let mut entries = Vec::new();
        let mut by_path = BTreeMap::new();
        let mut metadata_bytes = 0usize;
        for (source_index, source) in sources.iter().enumerate() {
            let mut zip = zip::ZipArchive::new(File::open(&source.path)?)?;
            if zip.len() > MAX_ZIP_ENTRIES {
                return Err(PackageError::Limit("APK has too many entries".into()));
            }
            for zip_index in 0..zip.len() {
                let entry = zip.by_index_raw(zip_index)?;
                let member_path = String::from_utf8_lossy(entry.name_raw()).into_owned();
                if entry.is_dir() {
                    continue;
                }
                if member_path.len() > MAX_PATH_BYTES || !safe_name(&member_path) {
                    if opts.strict {
                        return Err(PackageError::invalid(format!(
                            "unsafe APK member path in {}",
                            source.outer_name
                        )));
                    }
                    warnings.push(format!(
                        "skipped entry with unsafe path in {}: {}",
                        source.outer_name,
                        member_path.escape_debug()
                    ));
                    continue;
                }
                let path = format!("{}{member_path}", apks[source_index].path_prefix);
                metadata_bytes =
                    metadata_bytes.saturating_add(path.len() + source.outer_name.len() + 96);
                if metadata_bytes > MAX_CONTAINER_METADATA {
                    return Err(PackageError::Limit(
                        "archive inventory exceeds 32 MiB metadata limit".into(),
                    ));
                }
                if by_path.contains_key(&path) {
                    if opts.strict {
                        return Err(PackageError::invalid(format!(
                            "duplicate APK member path: {path}"
                        )));
                    }
                    warnings.push(format!("skipped duplicate entry: {path}"));
                    continue;
                }
                let index = entries.len();
                by_path.insert(path.clone(), index);
                entries.push(ContainerEntry {
                    index,
                    path,
                    source: source.outer_name.clone(),
                    member_path,
                    size: entry.size(),
                    compressed_size: entry.compressed_size(),
                    crc32: entry.crc32(),
                    is_apk_member: true,
                    source_index,
                    zip_index,
                });
            }
        }

        if kind == PackageKind::Xapk {
            let source_index = sources.len();
            sources.push(Source {
                path: path.clone(),
                outer_name: "container".into(),
                _temporary: None,
            });
            for zip_index in 0..outer.len() {
                let entry = outer.by_index_raw(zip_index)?;
                if entry.is_dir() {
                    continue;
                }
                let member_path = String::from_utf8_lossy(entry.name_raw()).into_owned();
                if member_path.to_ascii_lowercase().ends_with(".apk")
                    || member_path.len() > MAX_PATH_BYTES
                    || !safe_name(&member_path)
                {
                    continue;
                }
                let logical = format!("container/{member_path}");
                metadata_bytes = metadata_bytes.saturating_add(logical.len() + 96);
                if metadata_bytes > MAX_CONTAINER_METADATA {
                    return Err(PackageError::Limit(
                        "archive inventory exceeds 32 MiB metadata limit".into(),
                    ));
                }
                if by_path.contains_key(&logical) {
                    if opts.strict {
                        return Err(PackageError::invalid(format!(
                            "duplicate logical package path: {logical}"
                        )));
                    }
                    warnings.push(format!("skipped duplicate entry: {logical}"));
                    continue;
                }
                let index = entries.len();
                by_path.insert(logical.clone(), index);
                entries.push(ContainerEntry {
                    index,
                    path: logical,
                    source: "container".into(),
                    member_path,
                    size: entry.size(),
                    compressed_size: entry.compressed_size(),
                    crc32: entry.crc32(),
                    is_apk_member: false,
                    source_index,
                    zip_index,
                });
            }
        }
        apks.shrink_to_fit();
        Ok(Arc::new(Self {
            path,
            kind,
            original_size: metadata.len(),
            original_modified: metadata.modified().ok(),
            sources,
            apks,
            entries,
            by_path,
            summary,
            warnings,
        }))
    }

    /// Inventory of the outer archive: validates names and collects the APK members.
    fn scan_outer(
        outer: &mut zip::ZipArchive<File>,
        opts: &OpenOptions,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<Candidate>> {
        let mut candidates = Vec::new();
        let mut names = BTreeSet::new();
        for index in 0..outer.len() {
            let entry = outer.by_index_raw(index)?;
            let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
            if entry.is_dir() {
                continue;
            }
            if name.len() > MAX_PATH_BYTES || !safe_name(name.trim_end_matches('/')) {
                if opts.strict {
                    return Err(PackageError::invalid("unsafe container entry path"));
                }
                warnings.push(format!(
                    "skipped container entry with unsafe path: {}",
                    name.escape_debug()
                ));
                continue;
            }
            if opts.strict && !names.insert(name.to_ascii_lowercase()) {
                return Err(PackageError::invalid(
                    "duplicate or case-colliding container entry",
                ));
            }
            // the reference plugins match `.apk` case sensitively, the strict mode does not
            let is_apk = if opts.strict {
                name.to_ascii_lowercase().ends_with(".apk")
            } else {
                name.ends_with(".apk")
            };
            if is_apk {
                candidates.push(Candidate {
                    zip_index: index,
                    name,
                    size: entry.size(),
                });
            }
        }
        ensure!(
            !candidates.is_empty(),
            "package container contains no APK members"
        );
        if candidates.len() > MAX_NESTED_APKS {
            return Err(PackageError::Limit(
                "package container has too many APK members".into(),
            ));
        }
        Ok(candidates)
    }

    /// Reduce the APK members to the set that makes up the package.
    fn select_candidates(
        outer: &mut zip::ZipArchive<File>,
        kind: PackageKind,
        opts: &OpenOptions,
        candidates: &mut Vec<Candidate>,
        summary: &mut Option<String>,
        warnings: &mut Vec<String>,
    ) -> Result<()> {
        let find = |outer: &mut zip::ZipArchive<File>, name: &str| -> Option<usize> {
            (0..outer.len()).find(|&i| {
                outer
                    .by_index_raw(i)
                    .is_ok_and(|e| e.name_raw() == name.as_bytes())
            })
        };
        match kind {
            PackageKind::Apks => {
                let Some(toc_index) = find(outer, "toc.pb") else {
                    ensure!(!opts.strict, "APKS archive has no toc.pb");
                    return Ok(());
                };
                let toc = match read_zip_entry(outer, toc_index, 16 * 1024 * 1024)
                    .and_then(|b| parse_toc(&b))
                {
                    Ok(t) => t,
                    Err(e) if !opts.strict => {
                        warnings.push(format!("ignored unreadable toc.pb: {e}"));
                        return Ok(());
                    }
                    Err(e) => return Err(e),
                };
                if !opts.strict {
                    return Ok(());
                }
                let mut numbers = BTreeSet::new();
                ensure!(
                    toc.variants.iter().all(|v| numbers.insert(v.number)),
                    "APKS toc.pb has duplicate variant numbers"
                );
                let (variant, paths) = toc.select_variant()?;
                let mut selected = BTreeSet::new();
                for name in paths {
                    ensure!(
                        safe_name(name) && selected.insert(name.to_ascii_lowercase()),
                        "APKS variant references unsafe or duplicate APK path"
                    );
                    ensure!(
                        candidates.iter().any(|c| &c.name == name),
                        "APKS variant references missing APK: {name}"
                    );
                }
                candidates.retain(|c| selected.contains(&c.name.to_ascii_lowercase()));
                *summary = Some(format!(
                    "Bundletool variant {} analysis union (not a device install set), package {}, {} APKs",
                    variant.number,
                    toc.package,
                    paths.len()
                ));
            }
            PackageKind::Xapk => {
                let json = find(outer, "manifest.json")
                    .map(|i| read_zip_entry(outer, i, 2 * 1024 * 1024))
                    .transpose()?
                    .and_then(|b| Json::parse(&String::from_utf8_lossy(&b)).ok());
                let declared: Option<Vec<String>> = json.as_ref().and_then(|j| {
                    let version = j.get("xapk_version").and_then(Json::as_i64).unwrap_or(0);
                    let apks = j.get("split_apks")?.as_array()?;
                    if version != 2 && !opts.strict {
                        return None;
                    }
                    let names: Vec<String> = apks
                        .iter()
                        .filter_map(|a| {
                            a.get("file")
                                .or_else(|| a.get("name"))?
                                .as_str()
                                .map(str::to_owned)
                        })
                        .collect();
                    (!names.is_empty() || opts.strict).then_some(names)
                });
                if let Some(names) = &declared {
                    if opts.strict {
                        let mut seen = BTreeSet::new();
                        for name in names {
                            ensure!(
                                safe_name(name) && seen.insert(name.to_ascii_lowercase()),
                                "unsafe or duplicate XAPK APK reference"
                            );
                            ensure!(
                                candidates.iter().any(|c| c.name.eq_ignore_ascii_case(name)),
                                "XAPK manifest references a missing APK: {name}"
                            );
                        }
                    }
                    // the reference loads exactly the declared APKs of a version 2 manifest
                    candidates.retain(|c| names.iter().any(|n| n == &c.name));
                } else if !opts.strict {
                    return Err(PackageError::invalid(
                        "XAPK archive has no readable version 2 manifest.json listing split APKs",
                    ));
                } else if let Some(name) = json
                    .as_ref()
                    .and_then(|j| j.get("package_name"))
                    .and_then(Json::as_str)
                {
                    let wanted = format!("{name}.apk");
                    if candidates.iter().any(|c| c.name == wanted) {
                        // single APK package without split list
                        candidates.retain(|c| c.name == wanted);
                    }
                }
            }
            PackageKind::Apkm => {
                let info = find(outer, "info.json")
                    .map(|i| read_zip_entry(outer, i, 2 * 1024 * 1024))
                    .transpose()?
                    .and_then(|b| Json::parse(&String::from_utf8_lossy(&b)).ok());
                let supported = info
                    .as_ref()
                    .and_then(|j| j.get("apkm_version"))
                    .and_then(Json::as_i64)
                    .is_some_and(|v| v != -1);
                ensure!(
                    supported,
                    "APKM archive has no info.json with an apkm_version"
                );
            }
            PackageKind::Apk => {}
        }
        Ok(())
    }

    /// Read the identity of every APK, order the base APK first and decide the path prefixes.
    fn identify(
        kind: PackageKind,
        opts: &OpenOptions,
        sources: &mut [Source],
        warnings: &mut Vec<String>,
        summary: &mut Option<String>,
    ) -> Result<Vec<ApkInfo>> {
        let mut infos: Vec<ApkInfo> = Vec::with_capacity(sources.len());
        for source in sources.iter() {
            let mut meta = None;
            if kind != PackageKind::Apk {
                let mut zip = zip::ZipArchive::new(File::open(&source.path)?)?;
                let idx = (0..zip.len()).find(|&i| {
                    zip.by_index_raw(i)
                        .is_ok_and(|e| e.name_raw() == b"AndroidManifest.xml")
                });
                let parsed = match idx {
                    Some(i) => read_zip_entry(&mut zip, i, 32 * 1024 * 1024)
                        .and_then(|b| parse_manifest_meta(&b)),
                    None => Err(PackageError::invalid(
                        "nested APK lacks AndroidManifest.xml",
                    )),
                };
                match parsed {
                    Ok(m) => meta = Some(m),
                    Err(e) if opts.strict => {
                        return Err(PackageError::invalid(format!(
                            "identifying {}: {e}",
                            source.outer_name
                        )))
                    }
                    Err(e) => warnings.push(format!(
                        "cannot read the manifest of {}: {e}",
                        source.outer_name
                    )),
                }
            }
            infos.push(ApkInfo {
                name: source.outer_name.clone(),
                split: meta.as_ref().and_then(|m| m.split.clone()),
                meta,
                path_prefix: String::new(),
            });
        }
        if kind == PackageKind::Apk {
            return Ok(infos);
        }

        let bases: Vec<usize> = (0..infos.len())
            .filter(|&i| infos[i].meta.as_ref().is_some_and(|m| m.split.is_none()))
            .collect();
        if opts.strict {
            ensure!(
                bases.len() == 1,
                "container has {} base APK alternatives; provide a device-targeted package with one base",
                bases.len()
            );
            Self::validate_identities(&infos)?;
            ensure!(
                !(infos.len() > 1 && infos[bases[0]].name.contains("standalone")),
                "package contains standalone and split alternatives; provide one device-targeted variant"
            );
        }
        // the base APK comes first; without a recognisable base the first APK acts as one
        let base = bases.first().copied().unwrap_or(0);
        if base != 0 {
            sources.swap(0, base);
            infos.swap(0, base);
        }
        let mut used: BTreeSet<String> = BTreeSet::new();
        for (i, info) in infos.iter_mut().enumerate() {
            if i == 0 {
                continue;
            }
            let mut name = info
                .meta
                .as_ref()
                .and_then(|m| m.split.clone())
                .filter(|s| safe_split(s))
                .unwrap_or_else(|| file_stem(&info.name));
            if !used.insert(name.to_ascii_lowercase()) {
                let mut n = 2;
                while !used.insert(format!("{}_{n}", name.to_ascii_lowercase())) {
                    n += 1;
                }
                name = format!("{name}_{n}");
            }
            info.path_prefix = format!("splits/{name}/");
            info.split = Some(name);
        }
        if summary.is_none() {
            *summary = Some(format!("{:?} analysis union: {} APKs", kind, infos.len()));
        }
        Ok(infos)
    }

    fn validate_identities(infos: &[ApkInfo]) -> Result<()> {
        let base = infos
            .iter()
            .find(|i| i.meta.as_ref().is_some_and(|m| m.split.is_none()))
            .and_then(|i| i.meta.as_ref())
            .ok_or_else(|| PackageError::invalid("no base APK"))?;
        let mut split_names = BTreeSet::new();
        for info in infos {
            let meta = info
                .meta
                .as_ref()
                .ok_or_else(|| PackageError::invalid("APK without identity"))?;
            ensure!(
                meta.package == base.package && meta.version_code == base.version_code,
                "split {} has a different package/version identity",
                info.name
            );
            if let Some(split) = &meta.split {
                ensure!(
                    safe_split(split) && split_names.insert(split.to_ascii_lowercase()),
                    "duplicate or unsafe split identity: {split}"
                );
                if let Some(parent) = meta.config_for_split.as_deref().filter(|p| !p.is_empty()) {
                    ensure!(
                        parent == "base"
                            || infos.iter().any(|c| c
                                .meta
                                .as_ref()
                                .is_some_and(|m| m.split.as_deref() == Some(parent))),
                        "config split {split} has missing feature parent {parent}"
                    );
                }
            }
        }
        Ok(())
    }

    pub fn kind(&self) -> PackageKind {
        self.kind
    }

    /// Path of the opened file.
    pub fn source_path(&self) -> &Path {
        &self.path
    }

    /// One line description of the selection that was made, if any.
    pub fn selection_summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    /// Non-fatal problems met while opening (skipped entries, unreadable manifests).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn entries(&self) -> &[ContainerEntry] {
        &self.entries
    }

    pub fn entry(&self, path: &str) -> Option<&ContainerEntry> {
        self.by_path.get(path).map(|&i| &self.entries[i])
    }

    /// The contained APKs, base first.
    pub fn apks(&self) -> &[ApkInfo] {
        &self.apks
    }

    /// Readable files of the contained APKs (temporary files for nested ones), base first. The
    /// files exist as long as the container is alive.
    pub fn apk_files(&self) -> Vec<&Path> {
        self.sources
            .iter()
            .filter(|s| s.outer_name != "container")
            .map(|s| s.path.as_path())
            .collect()
    }

    fn check_original(&self) -> Result<()> {
        let metadata = File::open(&self.path)?.metadata()?;
        if metadata.len() != self.original_size
            || metadata.modified().ok() != self.original_modified
        {
            return Err(PackageError::Changed(self.path.display().to_string()));
        }
        Ok(())
    }

    /// Copy the member with the given entry index to `writer`, failing above `max_bytes`.
    pub fn copy_member(&self, index: usize, writer: &mut impl Write, max_bytes: u64) -> Result<()> {
        let expected = self
            .entries
            .get(index)
            .ok_or_else(|| PackageError::NotFound(format!("member index {index}")))?;
        if expected.size > max_bytes {
            return Err(PackageError::Limit(format!(
                "member {} exceeds {max_bytes} bytes",
                expected.path
            )));
        }
        self.check_original()?;
        let source = &self.sources[expected.source_index];
        let mut zip = zip::ZipArchive::new(File::open(&source.path)?)?;
        let mut entry = zip.by_index(expected.zip_index)?;
        ensure!(
            entry.name_raw() == expected.member_path.as_bytes()
                && entry.size() == expected.size
                && entry.crc32() == expected.crc32,
            "archive member changed: {}",
            expected.path
        );
        let copied = std::io::copy(&mut entry.by_ref().take(max_bytes + 1), writer)?;
        ensure!(
            copied == expected.size && copied <= max_bytes,
            "archive member decompression size mismatch: {}",
            expected.path
        );
        Ok(())
    }

    pub fn read_member(&self, index: usize, max_bytes: u64) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.copy_member(index, &mut bytes, max_bytes)?;
        Ok(bytes)
    }

    /// Read a member by logical path.
    pub fn read_path(&self, path: &str, max_bytes: u64) -> Result<Vec<u8>> {
        let index = *self
            .by_path
            .get(path)
            .ok_or_else(|| PackageError::NotFound(path.to_owned()))?;
        self.read_member(index, max_bytes)
    }

    /// Read at most `limit` leading bytes of a member (for magic checks).
    pub fn read_prefix(&self, index: usize, limit: usize) -> Result<Vec<u8>> {
        let expected = self
            .entries
            .get(index)
            .ok_or_else(|| PackageError::NotFound(format!("member index {index}")))?;
        self.check_original()?;
        let source = &self.sources[expected.source_index];
        let mut zip = zip::ZipArchive::new(File::open(&source.path)?)?;
        let mut entry = zip.by_index(expected.zip_index)?;
        let mut bytes = Vec::new();
        entry.by_ref().take(limit as u64).read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use std::io::Cursor;

    pub fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    /// A temporary file removed on drop.
    pub struct TempFile(pub PathBuf);
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    pub fn temp_container(extension: &str, entries: &[(&str, &[u8])]) -> TempFile {
        let (file, tmp) = temporary_apk().unwrap();
        drop(file);
        let path = tmp.0.with_extension(extension);
        fs::rename(&tmp.0, &path).unwrap();
        std::mem::forget(tmp);
        fs::write(&path, zip_bytes(entries)).unwrap();
        TempFile(path)
    }

    pub fn manifest(split: Option<&str>) -> Vec<u8> {
        match split {
            Some(s) => format!(
                "<manifest package=\"sample.test\" android:versionCode=\"1\" split=\"{s}\"/>"
            )
            .into_bytes(),
            None => br#"<manifest package="sample.test" android:versionCode="1"/>"#.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;
    use crate::toc::enc;

    fn base() -> Vec<u8> {
        zip_bytes(&[
            ("AndroidManifest.xml", &manifest(None)),
            ("classes.dex", b"dex-base"),
        ])
    }

    fn split(id: &str) -> Vec<u8> {
        zip_bytes(&[
            ("AndroidManifest.xml", &manifest(Some(id))),
            ("res/raw/note.txt", b"split asset"),
        ])
    }

    const XAPK_MANIFEST: &[u8] =
        br#"{"xapk_version":2,"package_name":"sample.test","split_apks":[{"file":"base.apk","id":"base"},{"file":"feature.apk","id":"feature"}]}"#;

    #[test]
    fn xapk_exposes_splits_and_outer_files_under_logical_paths() {
        let (b, f) = (base(), split("feature"));
        let t = temp_container(
            "xapk",
            &[
                ("manifest.json", XAPK_MANIFEST),
                ("feature.apk", &f),
                ("base.apk", &b),
                ("main.obb", b"opaque"),
            ],
        );
        for strict in [false, true] {
            let c = PackageContainer::open(&t.0, &OpenOptions { strict }).unwrap();
            assert_eq!(c.kind(), PackageKind::Xapk);
            // base first even though it is listed second
            assert_eq!(c.apks()[0].name, "base.apk");
            assert!(c.apks()[0].path_prefix.is_empty());
            assert_eq!(c.apks()[1].path_prefix, "splits/feature/");
            assert_eq!(c.read_path("classes.dex", 64).unwrap(), b"dex-base");
            assert_eq!(
                c.read_path("splits/feature/res/raw/note.txt", 64).unwrap(),
                b"split asset"
            );
            assert_eq!(c.read_path("container/main.obb", 64).unwrap(), b"opaque");
            assert!(c.read_path("container/manifest.json", 4096).is_ok());
            let e = c.entry("splits/feature/res/raw/note.txt").unwrap();
            assert_eq!(e.reference_path(), "feature.apk/res/raw/note.txt");
            assert_eq!(c.apk_files().len(), 2);
            assert!(c.read_path("main.obb", 64).is_err());
            assert!(c.read_path("classes.dex", 3).is_err());
        }
    }

    #[test]
    fn temporary_files_are_removed_on_drop() {
        let b = base();
        let t = temp_container(
            "xapk",
            &[
                (
                    "manifest.json",
                    br#"{"xapk_version":2,"split_apks":[{"file":"base.apk"}]}"#,
                ),
                ("base.apk", &b),
            ],
        );
        let c = PackageContainer::open(&t.0, &OpenOptions::default()).unwrap();
        let tmp: Vec<PathBuf> = c.apk_files().iter().map(|p| p.to_path_buf()).collect();
        assert!(tmp[0].exists());
        drop(c);
        assert!(!tmp[0].exists());
    }

    #[test]
    fn xapk_without_version_two_manifest_is_not_a_package_by_default() {
        let b = base();
        let t = temp_container("xapk", &[("base.apk", &b)]);
        assert!(PackageContainer::open(&t.0, &OpenOptions::default()).is_err());
        let t = temp_container(
            "xapk",
            &[
                ("manifest.json", br#"{"xapk_version":1,"split_apks":[]}"#),
                ("base.apk", &b),
            ],
        );
        assert!(PackageContainer::open(&t.0, &OpenOptions::default()).is_err());
    }

    #[test]
    fn apks_reference_mode_takes_every_apk_strict_selects_one_variant() {
        let (b, f) = (base(), split("feature"));
        let mut toc = enc::bytes(4, b"sample.test");
        toc.extend(enc::variant(1, &[("old.apk", 4)]));
        toc.extend(enc::variant(3, &[("base.apk", 3), ("feature.apk", 3)]));
        toc.extend(enc::variant(9, &[("instant.apk", 5)]));
        let t = temp_container(
            "apks",
            &[
                ("toc.pb", &toc),
                ("old.apk", &b),
                ("base.apk", &b),
                ("feature.apk", &f),
                ("instant.apk", &b),
            ],
        );
        let strict = PackageContainer::open(&t.0, &OpenOptions { strict: true }).unwrap();
        assert!(strict.selection_summary().unwrap().contains("variant 3"));
        assert_eq!(strict.apks().len(), 2);
        assert!(!strict
            .entries()
            .iter()
            .any(|e| e.source == "old.apk" || e.source == "instant.apk"));
        // reference mode: all four APKs, the duplicate bases get unique prefixes
        let lenient = PackageContainer::open(&t.0, &OpenOptions::default()).unwrap();
        assert_eq!(lenient.apks().len(), 4);
        let prefixes: BTreeSet<_> = lenient
            .apks()
            .iter()
            .map(|a| a.path_prefix.clone())
            .collect();
        assert_eq!(prefixes.len(), 4);
    }

    #[test]
    fn apkm_requires_info_json() {
        let (b, f) = (base(), split("config.en"));
        let t = temp_container(
            "apkm",
            &[
                ("info.json", br#"{"apkm_version":6}"#),
                ("base.apk", &b),
                ("split_config.en.apk", &f),
            ],
        );
        let c = PackageContainer::open(&t.0, &OpenOptions::default()).unwrap();
        assert_eq!(c.kind(), PackageKind::Apkm);
        assert!(c.entry("splits/config.en/res/raw/note.txt").is_some());
        let t = temp_container("apkm", &[("base.apk", &b)]);
        assert!(PackageContainer::open(&t.0, &OpenOptions::default()).is_err());
    }

    #[test]
    fn strict_mode_rejects_ambiguous_and_malformed_containers() {
        let b = base();
        let t = temp_container("apks", &[("toc.pb", &[0xff]), ("base.apk", &b)]);
        assert!(PackageContainer::open(&t.0, &OpenOptions { strict: true }).is_err());
        // reference mode ignores a broken toc.pb
        assert!(PackageContainer::open(&t.0, &OpenOptions::default()).is_ok());
        let t = temp_container("xapk", &[("base.apk", &b), ("alternative.apk", &b)]);
        let err = PackageContainer::open(&t.0, &OpenOptions { strict: true }).unwrap_err();
        assert!(err.to_string().contains("base APK alternatives"), "{err}");
        let other = zip_bytes(&[(
            "AndroidManifest.xml",
            br#"<manifest package="other.pkg" android:versionCode="1" split="x"/>"#,
        )]);
        let t = temp_container("xapk", &[("base.apk", &b), ("x.apk", &other)]);
        let err = PackageContainer::open(&t.0, &OpenOptions { strict: true }).unwrap_err();
        assert!(err.to_string().contains("different package"), "{err}");
    }

    #[test]
    fn outer_file_cannot_shadow_logical_apk_member() {
        let inner = zip_bytes(&[
            ("AndroidManifest.xml", &manifest(None)),
            ("container/main.obb", b"inner"),
        ]);
        let t = temp_container(
            "xapk",
            &[
                (
                    "manifest.json",
                    br#"{"xapk_version":2,"split_apks":[{"file":"base.apk"}]}"#,
                ),
                ("base.apk", &inner),
                ("main.obb", b"outer"),
            ],
        );
        let err = PackageContainer::open(&t.0, &OpenOptions { strict: true }).unwrap_err();
        assert!(
            err.to_string().contains("duplicate logical package path"),
            "{err}"
        );
        let c = PackageContainer::open(&t.0, &OpenOptions::default()).unwrap();
        assert_eq!(c.read_path("container/main.obb", 16).unwrap(), b"inner");
        assert!(c.warnings().iter().any(|w| w.contains("duplicate")));
    }

    #[test]
    fn plain_apk_has_one_source_and_ignores_the_manifest() {
        let t = temp_container(
            "apk",
            &[("AndroidManifest.xml", b"bad XML"), ("classes.dex", b"x")],
        );
        let c = PackageContainer::open(&t.0, &OpenOptions::default()).unwrap();
        assert_eq!(c.kind(), PackageKind::Apk);
        assert_eq!(c.entries().len(), 2);
        assert_eq!(c.apk_files().len(), 1);
    }

    #[test]
    fn rejects_non_zip_input() {
        let t = temp_container("apk", &[]);
        fs::write(&t.0, b"not a zip at all, really not a zip file").unwrap();
        assert!(PackageContainer::open(&t.0, &OpenOptions::default()).is_err());
    }
}
