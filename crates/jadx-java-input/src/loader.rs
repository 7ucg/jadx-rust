//! Locate and load class files from `.class` files and from `.jar`/`.zip` archives (archives
//! nested in archives are unpacked too).

use std::io::{Cursor, Read};
use std::path::Path;

use crate::class::JavaClass;
use crate::error::Result;

const CLASS_MAGIC: [u8; 4] = [0xCA, 0xFE, 0xBA, 0xBE];
const ZIP_MAGIC: [u8; 4] = [0x50, 0x4B, 0x03, 0x04];

/// Maximum number of entries of an archive (larger archives are rejected).
const MAX_ENTRIES: usize = 100_000;
/// Entries that expand more than this factor above `ZIP_BOMB_MIN_SIZE` are ignored.
const ZIP_BOMB_FACTOR: u64 = 100;
const ZIP_BOMB_MIN_SIZE: u64 = 25 * 1024 * 1024;

/// Result of loading an input: the classes plus non-fatal problems (bad entries are skipped,
/// never fatal for the whole input).
#[derive(Debug, Default)]
pub struct LoadResult {
    pub classes: Vec<JavaClass>,
    pub warnings: Vec<String>,
}

fn is_zip_ext(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((_, ext)) => matches!(ext, "zip" | "jar" | "apk"),
        None => false,
    }
}

fn concat_source(parent: Option<&str>, name: &str) -> String {
    match parent {
        None => name.to_string(),
        Some(p) => format!("{p}:{name}"),
    }
}

/// Load all classes from the file at `path`. The class source name is the file name.
pub fn load_path(path: &Path) -> Result<LoadResult> {
    let content = std::fs::read(path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    Ok(load_bytes(&name, content))
}

/// Load classes from an in-memory `.class` file or archive.
pub fn load_bytes(name: &str, content: Vec<u8>) -> LoadResult {
    let mut res = LoadResult::default();
    if content.len() < 4 {
        return res;
    }
    if content[..4] == CLASS_MAGIC || name.ends_with(".class") {
        add_class(&mut res, name.to_string(), content);
        return res;
    }
    if content[..4] == ZIP_MAGIC || is_zip_ext(name) {
        collect_from_zip(&mut res, name, content);
    }
    res
}

fn add_class(res: &mut LoadResult, source: String, data: Vec<u8>) {
    match JavaClass::parse(source.clone(), data) {
        Ok(c) => res.classes.push(c),
        Err(e) => res
            .warnings
            .push(format!("Failed to load class data for file: {source}: {e}")),
    }
}

fn valid_entry_name(name: &str) -> bool {
    if name.contains("../") || name.contains("..\\") {
        return false;
    }
    !(name.starts_with('/') || name.starts_with('\\'))
}

fn collect_from_zip(res: &mut LoadResult, name: &str, content: Vec<u8>) {
    let mut archive = match zip::ZipArchive::new(Cursor::new(content)) {
        Ok(a) => a,
        Err(e) => {
            res.warnings
                .push(format!("Failed to process zip file: {name}: {e}"));
            return;
        }
    };
    if archive.len() > MAX_ENTRIES {
        res.warnings.push(format!(
            "Failed to process zip file: {name}: too many entries"
        ));
        return;
    }
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
        if !valid_entry_name(&entry_name) {
            continue;
        }
        let (size, csize) = (entry.size(), entry.compressed_size());
        if size >= ZIP_BOMB_MIN_SIZE && csize * ZIP_BOMB_FACTOR < size {
            continue;
        }
        if entry_name.starts_with("META-INF/versions/") {
            // classes for other java versions
            continue;
        }
        let mut buf = Vec::with_capacity(size.min(1 << 26) as usize);
        if let Err(e) = entry.read_to_end(&mut buf) {
            res.warnings
                .push(format!("Failed to read zip entry: {entry_name}: {e}"));
            continue;
        }
        drop(entry);
        let source = concat_source(Some(name), &entry_name);
        if buf.starts_with(&CLASS_MAGIC) || entry_name.ends_with(".class") {
            add_class(res, source, buf);
        } else if buf.starts_with(&ZIP_MAGIC) || is_zip_ext(&entry_name) {
            collect_from_zip(res, &source, buf);
        }
    }
}
