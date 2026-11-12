//! Resource files of the inputs and their extraction into the resources directory.
//!
//! Non code files of zip based inputs are copied unchanged. The decoding of binary XML and of
//! the resource table is done by a [`ResourceDecoder`]; the default implementation uses the
//! `jadx-resources` crate.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::args::JadxArgs;
use crate::error::ErrorsCounter;
use crate::log::{Level, LogSink};
use crate::parallel::run_indexed;
use crate::paths::{prepare_file, safe_join, save_text};

/// Kind of a resource file, by file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceType {
    Code,
    Xml,
    Arsc,
    Apk,
    Font,
    Img,
    Archive,
    Videos,
    Sounds,
    Json,
    Text,
    Html,
    Lib,
    Manifest,
    UnknownBin,
    Unknown,
}

impl ResourceType {
    /// `ResourceType.getFileType`.
    pub fn from_name(name: &str) -> ResourceType {
        if name.ends_with("/resources.pb") {
            return ResourceType::Arsc;
        }
        let Some(dot) = name.rfind('.') else {
            return ResourceType::Unknown;
        };
        let ext = name[dot..].to_lowercase();
        use ResourceType::*;
        match ext.as_str() {
            ".dex" | ".jar" | ".class" => Code,
            ".xml" if name == "AndroidManifest.xml" => Manifest,
            ".xml" => Xml,
            ".arsc" => Arsc,
            ".apk" | ".apkm" | ".apks" => Apk,
            ".ttf" | ".ttc" | ".otf" => Font,
            ".png" | ".gif" | ".jpg" | ".jpeg" | ".webp" | ".bmp" | ".tiff" => Img,
            ".zip" | ".rar" | ".7zip" | ".7z" | ".arj" | ".tar" | ".gzip" | ".bzip" | ".bzip2"
            | ".cab" | ".cpio" | ".ar" | ".gz" | ".tgz" | ".bz2" => Archive,
            ".mp4" | ".mkv" | ".webm" | ".avi" | ".flv" | ".3gp" => Videos,
            ".aac" | ".ogg" | ".opus" | ".mp3" | ".wav" | ".wma" | ".mid" | ".midi" => Sounds,
            ".json" => Json,
            ".txt" | ".ini" | ".conf" | ".yaml" | ".properties" | ".js" | ".java" | ".kt"
            | ".md" => Text,
            ".html" | ".htm" => Html,
            ".so" => Lib,
            ".bin" => UnknownBin,
            _ => Unknown,
        }
    }

    /// Types that the resource decoder turns into text files.
    pub fn is_decoded(self) -> bool {
        matches!(
            self,
            ResourceType::Manifest | ResourceType::Xml | ResourceType::Arsc
        )
    }
}

/// Where the content of a resource comes from.
#[derive(Debug, Clone)]
pub enum ResourceSource {
    /// Entry `index` of a zip archive.
    ZipEntry {
        archive: Arc<PathBuf>,
        index: usize,
        size: u64,
    },
    /// A plain file.
    File(PathBuf),
}

/// A resource of the input (`ResourceFile`).
#[derive(Debug, Clone)]
pub struct ResourceFile {
    name: String,
    deobf_name: Option<String>,
    ty: ResourceType,
    source: ResourceSource,
}

impl ResourceFile {
    pub fn new(name: impl Into<String>, ty: ResourceType, source: ResourceSource) -> Self {
        ResourceFile {
            name: name.into(),
            deobf_name: None,
            ty,
            source,
        }
    }

    /// Name in the input (the zip entry name).
    pub fn original_name(&self) -> &str {
        &self.name
    }

    /// Name used for the output file.
    pub fn deobf_name(&self) -> &str {
        self.deobf_name.as_deref().unwrap_or(&self.name)
    }

    pub fn set_deobf_name(&mut self, name: impl Into<String>) {
        self.deobf_name = Some(name.into());
    }

    /// `ResourceFile.setAlias`: the file is saved under the name of its resource table entry,
    /// the type follows the new name. Returns false if the name does not change.
    pub fn set_alias(&mut self, alias: &str) -> bool {
        if alias == self.name {
            return false;
        }
        self.deobf_name = Some(alias.to_string());
        self.ty = ResourceType::from_name(alias);
        true
    }

    pub fn resource_type(&self) -> ResourceType {
        self.ty
    }

    pub fn source(&self) -> &ResourceSource {
        &self.source
    }

    /// Read the content of the resource.
    pub fn load_bytes(&self) -> io::Result<Vec<u8>> {
        match &self.source {
            ResourceSource::File(p) => std::fs::read(p),
            ResourceSource::ZipEntry { archive, index, .. } => {
                let mut zip = open_zip(archive)?;
                let mut entry = zip.by_index(*index).map_err(zip_err)?;
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                Ok(buf)
            }
        }
    }
}

fn zip_err(e: zip::result::ZipError) -> io::Error {
    io::Error::other(e.to_string())
}

fn open_zip(path: &Path) -> io::Result<zip::ZipArchive<File>> {
    zip::ZipArchive::new(File::open(path)?).map_err(zip_err)
}

/// Maximum number of entries of a zip file that is accepted.
pub const ZIP_MAX_ENTRIES: usize = 100_000;

/// Result of the decoding of an archive.
#[derive(Debug, Default, Clone)]
pub struct DecodedResources {
    /// Decoded text files, output path relative to the resources directory.
    pub files: Vec<DecodedResFile>,
    /// Problems that did not stop the decoding. A message starting with `Error decode <name>`
    /// marks the entry `name` as not decoded: it is copied unchanged.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DecodedResFile {
    pub path: String,
    pub text: String,
}

/// Tables of the decompiled code that the resource decoder uses.
#[derive(Debug, Default, Clone)]
pub struct ResourceContext {
    /// Names of the constant fields of the app code by resource id (the fields of the app `R`
    /// class), used to name resources.
    pub code_names: HashMap<i32, String>,
    /// Renamed classes: raw name to the name in the decompiled code, for class names in XML.
    pub class_aliases: HashMap<String, String>,
}

/// Decodes the Android specific resources (manifest, binary XML, resource table) of an archive.
pub trait ResourceDecoder: Send + Sync {
    fn decode_archive(&self, archive: &Path, args: &JadxArgs) -> Result<DecodedResources, String>;

    /// Like [`decode_archive`](Self::decode_archive) with the tables of the decompiled code.
    fn decode_archive_with_context(
        &self,
        archive: &Path,
        args: &JadxArgs,
        context: &ResourceContext,
    ) -> Result<DecodedResources, String> {
        let _ = context;
        self.decode_archive(archive, args)
    }
}

/// Options of the resource decoder crate from the decompiler arguments.
pub fn resource_options(args: &JadxArgs) -> jadx_resources::ResOptions {
    use crate::args::ResourceNameSource as Src;
    jadx_resources::ResOptions {
        name_source: match args.resource_name_source {
            Src::Auto => jadx_resources::ResourceNameSource::Auto,
            Src::Resources => jadx_resources::ResourceNameSource::Resources,
            Src::Code => jadx_resources::ResourceNameSource::Code,
        },
        rename_valid: args.is_rename_valid(),
        rename_printable: args.is_rename_printable(),
        ..jadx_resources::ResOptions::default()
    }
}

/// Decoder based on the `jadx-resources` crate.
#[derive(Debug, Default, Clone, Copy)]
pub struct AndroidResourceDecoder;

impl ResourceDecoder for AndroidResourceDecoder {
    fn decode_archive(&self, archive: &Path, args: &JadxArgs) -> Result<DecodedResources, String> {
        self.decode_archive_with_context(archive, args, &ResourceContext::default())
    }

    fn decode_archive_with_context(
        &self,
        archive: &Path,
        args: &JadxArgs,
        context: &ResourceContext,
    ) -> Result<DecodedResources, String> {
        let mut opts = jadx_resources::ApkOptions {
            res: resource_options(args),
            ..jadx_resources::ApkOptions::default()
        };
        opts.res.code_names = context.code_names.clone();
        opts.skip_xml_pretty_print = args.skip_xml_pretty_print;
        let lookup = |name: &str| context.class_aliases.get(name).cloned();
        if !context.class_aliases.is_empty() {
            opts.class_alias = Some(&lookup);
        }
        let res = jadx_resources::decode_apk(archive, &opts).map_err(|e| e.to_string())?;
        Ok(DecodedResources {
            files: res
                .files
                .into_iter()
                .map(|f| DecodedResFile {
                    path: f.path,
                    text: f.text,
                })
                .collect(),
            warnings: res.warnings,
        })
    }
}

fn is_zip_input(path: &Path, magic: &[u8]) -> bool {
    magic.starts_with(b"PK\x03\x04")
        || magic.starts_with(b"PK\x05\x06")
        || matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("zip" | "jar" | "apk")
        )
}

/// True if the file is a zip archive (by content or by extension).
pub fn is_zip_file(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    let n = File::open(path)
        .and_then(|mut f| f.read(&mut magic))
        .unwrap_or(0);
    is_zip_input(path, &magic[..n])
}

/// Collect the resources of the input files. Only entries of zip archives are resources;
/// directories and plain files are skipped. Problems are returned as warnings.
pub fn load_resources(inputs: &[PathBuf]) -> (Vec<ResourceFile>, Vec<String>) {
    let mut list = Vec::new();
    let mut warnings = Vec::new();
    for input in inputs {
        if input.is_dir() || !is_zip_file(input) {
            continue;
        }
        let mut zip = match open_zip(input) {
            Ok(z) => z,
            Err(e) => {
                warnings.push(format!("Failed to open zip file: {}: {e}", input.display()));
                continue;
            }
        };
        if zip.len() > ZIP_MAX_ENTRIES {
            warnings.push(format!(
                "Zip file has too many entries ({} > {}), resources skipped: {}",
                zip.len(),
                ZIP_MAX_ENTRIES,
                input.display()
            ));
            continue;
        }
        let archive = Arc::new(input.clone());
        for index in 0..zip.len() {
            let entry = match zip.by_index_raw(index) {
                Ok(e) => e,
                Err(e) => {
                    warnings.push(format!(
                        "Failed to read zip entry #{index} of {}: {e}",
                        input.display()
                    ));
                    continue;
                }
            };
            if entry.is_dir() {
                continue;
            }
            let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
            if !jadx_resources::apk::is_safe_output_path(&name) {
                warnings.push(format!("Skip resource with invalid name: {name:?}"));
                continue;
            }
            let ty = ResourceType::from_name(&name);
            list.push(ResourceFile::new(
                name,
                ty,
                ResourceSource::ZipEntry {
                    archive: archive.clone(),
                    index,
                    size: entry.size(),
                },
            ));
        }
    }
    (list, warnings)
}

fn archive_of(f: &ResourceFile) -> Option<&Arc<PathBuf>> {
    match &f.source {
        ResourceSource::ZipEntry { archive, .. } => Some(archive),
        ResourceSource::File(_) => None,
    }
}

/// Settings of [`save_resources`].
pub struct SaveResources<'a> {
    pub args: &'a JadxArgs,
    pub out_dir: &'a Path,
    pub decoder: Option<&'a dyn ResourceDecoder>,
    /// Tables of the decompiled code for the decoder.
    pub context: &'a ResourceContext,
    pub log: &'a dyn LogSink,
    pub errors: &'a ErrorsCounter,
    /// Called with `(done, total)` after every written file.
    pub progress: Option<&'a (dyn Fn(usize, usize) + Sync)>,
}

/// Write the resources below `cfg.out_dir`: decoded files first, then all other entries copied
/// unchanged. Output paths are validated; nothing is written outside the directory.
pub fn save_resources(files: &[ResourceFile], cfg: &SaveResources<'_>) {
    // archives with an Android manifest or resource table are decoded
    let mut decoded_paths: HashSet<String> = HashSet::new();
    let mut failed: HashSet<(PathBuf, String)> = HashSet::new();
    let mut decoded_archives: HashSet<PathBuf> = HashSet::new();
    if let Some(decoder) = cfg.decoder {
        let mut seen: Vec<&Arc<PathBuf>> = Vec::new();
        for f in files {
            let Some(archive) = archive_of(f) else {
                continue;
            };
            if seen.iter().any(|a| a.as_path() == archive.as_path()) {
                continue;
            }
            seen.push(archive);
            let has_android = files.iter().any(|g| {
                archive_of(g).is_some_and(|a| a.as_path() == archive.as_path())
                    && matches!(g.ty, ResourceType::Manifest | ResourceType::Arsc)
            });
            if !has_android {
                continue;
            }
            match decoder.decode_archive_with_context(archive, cfg.args, cfg.context) {
                Ok(res) => {
                    decoded_archives.insert(archive.as_path().to_path_buf());
                    for w in &res.warnings {
                        cfg.log.log(Level::Warn, w);
                        if let Some(name) = w.strip_prefix("Error decode ") {
                            let name = name.split(':').next().unwrap_or(name).trim();
                            failed.insert((archive.as_path().to_path_buf(), name.to_string()));
                        }
                    }
                    for d in res.files {
                        let Some(out) = safe_join(cfg.out_dir, &d.path) else {
                            cfg.log.log(
                                Level::Error,
                                &format!(
                                    "Invalid resource name or path traversal attack detected: {}",
                                    d.path
                                ),
                            );
                            continue;
                        };
                        if let Err(e) = save_text(&out, &d.text) {
                            cfg.log.log(
                                Level::Warn,
                                &format!("Failed to save resource: {}: {e}", d.path),
                            );
                            cfg.errors
                                .add_error(&format!("resource {}", d.path), &e.to_string());
                        } else {
                            decoded_paths.insert(d.path);
                        }
                    }
                }
                Err(e) => cfg.log.log(
                    Level::Error,
                    &format!("Failed to decode resources of {}: {e}", archive.display()),
                ),
            }
        }
    }

    // plain copies
    let todo: Vec<&ResourceFile> = files
        .iter()
        .filter(|f| {
            if f.ty == ResourceType::Code {
                // code files are loaded as classes, not saved as resources
                return false;
            }
            if f.ty.is_decoded() {
                if let Some(a) = archive_of(f) {
                    if decoded_archives.contains(a.as_path())
                        && !failed.contains(&(a.as_path().to_path_buf(), f.name.clone()))
                    {
                        return false;
                    }
                }
            }
            !decoded_paths.contains(f.deobf_name())
        })
        .collect();
    let total = todo.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    run_indexed(
        total,
        cfg.args.threads_count,
        || None::<(PathBuf, zip::ZipArchive<File>)>,
        |open, i| {
            let f = todo[i];
            if let Err(e) = copy_resource(f, cfg.out_dir, open) {
                cfg.log.log(
                    Level::Warn,
                    &format!("Failed to save resource: {}: {e}", f.name),
                );
            }
            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if let Some(p) = cfg.progress {
                p(n, total);
            }
        },
    );
}

fn copy_resource(
    f: &ResourceFile,
    out_dir: &Path,
    open: &mut Option<(PathBuf, zip::ZipArchive<File>)>,
) -> io::Result<()> {
    let out = safe_join(out_dir, f.deobf_name()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid resource name or path traversal attack detected",
        )
    })?;
    prepare_file(&out)?;
    let result = match &f.source {
        ResourceSource::File(p) => std::fs::copy(p, &out).map(|_| ()),
        ResourceSource::ZipEntry { archive, index, .. } => {
            if open
                .as_ref()
                .map(|(p, _)| p != archive.as_path())
                .unwrap_or(true)
            {
                *open = Some((archive.as_path().to_path_buf(), open_zip(archive)?));
            }
            let zip = &mut open.as_mut().expect("archive opened").1;
            let mut entry = zip.by_index(*index).map_err(zip_err)?;
            let mut dst = File::create(&out)?;
            io::copy(&mut entry, &mut dst).map(|_| ())
        }
    };
    if result.is_err() {
        // do not keep a partially written file
        let _ = std::fs::remove_file(&out);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_by_name() {
        assert_eq!(
            ResourceType::from_name("AndroidManifest.xml"),
            ResourceType::Manifest
        );
        assert_eq!(
            ResourceType::from_name("res/layout/a.xml"),
            ResourceType::Xml
        );
        assert_eq!(
            ResourceType::from_name("resources.arsc"),
            ResourceType::Arsc
        );
        assert_eq!(ResourceType::from_name("classes2.dex"), ResourceType::Code);
        assert_eq!(
            ResourceType::from_name("lib/arm64-v8a/liba.so"),
            ResourceType::Lib
        );
        assert_eq!(
            ResourceType::from_name("assets/blob"),
            ResourceType::Unknown
        );
        assert_eq!(ResourceType::from_name("res/a.PNG"), ResourceType::Img);
    }
}
