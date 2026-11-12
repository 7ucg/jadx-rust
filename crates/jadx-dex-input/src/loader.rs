//! Locate and load DEX files from a `.dex` file, a v41 container or a zip (`.apk`/`.jar`/`.zip`).

use std::path::Path;
use std::sync::Arc;

use crate::error::{DexError, Result};
use crate::file::{verify_checksum, DexFile};
use crate::header::{CONTAINER_HEADER_SIZE, DEX_MAGIC, ZIP_MAGIC};
use crate::reader::Reader;

#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// Verify the Adler-32 checksum of every dex before loading (jadx default: on).
    pub verify_checksum: bool,
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions {
            verify_checksum: true,
        }
    }
}

/// Result of loading an input: the dex files plus non-fatal problems (bad entries are skipped,
/// never fatal for the whole input — same policy as jadx).
#[derive(Debug, Default)]
pub struct LoadResult {
    pub dex_files: Vec<DexFile>,
    pub warnings: Vec<String>,
}

fn is_zip_ext(path: &str) -> bool {
    match path.rsplit_once('.') {
        Some((_, ext)) => matches!(ext, "zip" | "jar" | "apk"),
        None => false,
    }
}

/// Load all dex files from `path`.
pub fn load_path(path: &Path, opts: &LoadOptions) -> Result<LoadResult> {
    let content = std::fs::read(path)?;
    let name = path.to_string_lossy().into_owned();
    load_bytes(&name, content, opts, true)
}

/// Load dex files from an in-memory buffer. `allow_zip` mirrors jadx: only top level inputs may
/// be zip archives, entries inside archives are not unpacked recursively.
pub fn load_bytes(
    name: &str,
    content: Vec<u8>,
    opts: &LoadOptions,
    allow_zip: bool,
) -> Result<LoadResult> {
    let mut res = LoadResult::default();
    if content.len() < 4 {
        return Ok(res);
    }
    if content[..4] == DEX_MAGIC {
        load_dex_readers(name, Arc::from(content), opts, &mut res)?;
        return Ok(res);
    }
    if name.ends_with(".dex") {
        res.warnings.push(format!(
            "Invalid DEX magic: 0x{} in file: {}",
            content[..4]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            name
        ));
    }
    if allow_zip && (content[..4] == ZIP_MAGIC || is_zip_ext(name)) {
        collect_from_zip(name, content, opts, &mut res);
    }
    Ok(res)
}

fn load_dex_readers(
    name: &str,
    content: Arc<[u8]>,
    opts: &LoadOptions,
    res: &mut LoadResult,
) -> Result<()> {
    let offsets = match sub_dex_offsets(&content)? {
        Some(list) => list,
        None => vec![0],
    };
    for off in offsets {
        if opts.verify_checksum {
            verify_checksum(name, &content, off)?;
        }
        res.dex_files
            .push(DexFile::parse(name, content.clone(), off)?);
    }
    Ok(())
}

/// DEX v41 container: several dex structures concatenated in one file. Returns `None` for a
/// plain dex (standard header size).
fn sub_dex_offsets(content: &[u8]) -> Result<Option<Vec<usize>>> {
    let mut r = Reader::new(content, 36);
    let header_size = r.u32()?;
    if header_size < CONTAINER_HEADER_SIZE {
        return Ok(None);
    }
    let file_size = Reader::new(content, 32).u32()? as usize;
    let container_size = Reader::new(content, 112).u32()? as usize;
    let limit = container_size.min(content.len());
    let mut list = Vec::new();
    let mut start = 0usize;
    let mut end = file_size;
    loop {
        list.push(start);
        start = end;
        if start >= limit {
            break;
        }
        let next = Reader::new(content, start + 32).u32()? as usize;
        if next < CONTAINER_HEADER_SIZE as usize {
            return Err(DexError::BadContainer(format!(
                "sub dex at 0x{start:x} has invalid file size {next}"
            )));
        }
        end = start + next;
    }
    Ok(Some(list))
}

fn collect_from_zip(name: &str, content: Vec<u8>, opts: &LoadOptions, res: &mut LoadResult) {
    use std::io::{Cursor, Read};
    let mut archive = match zip::ZipArchive::new(Cursor::new(content)) {
        Ok(a) => a,
        Err(e) => {
            res.warnings
                .push(format!("Failed to process zip file: {name}: {e}"));
            return;
        }
    };
    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(e) => {
                res.warnings
                    .push(format!("Failed to read zip entry #{i} of {name}: {e}"));
                continue;
            }
        };
        if entry.is_dir() {
            continue;
        }
        let entry_name = String::from_utf8_lossy(entry.name_raw()).into_owned();
        let looks_like_dex = entry_name.ends_with(".dex");
        let mut buf = Vec::with_capacity(entry.size().min(1 << 26) as usize);
        if let Err(e) = entry.read_to_end(&mut buf) {
            res.warnings
                .push(format!("Failed to read zip entry: {entry_name}: {e}"));
            continue;
        }
        if buf.len() >= 4 && buf[..4] == DEX_MAGIC || looks_like_dex {
            let before = res.dex_files.len();
            let r = if buf.len() >= 4 && buf[..4] == DEX_MAGIC {
                load_dex_readers(&entry_name, Arc::from(buf), opts, res)
            } else {
                Err(DexError::BadMagic(
                    buf.get(..4)
                        .and_then(|s| s.try_into().ok())
                        .unwrap_or([0; 4]),
                ))
            };
            if let Err(e) = r {
                res.dex_files.truncate(before);
                res.warnings
                    .push(format!("Failed to read zip entry: {entry_name}: {e}"));
            }
        }
    }
}
