//! Load all code inputs of a path like the reference jadx does with its input plugins:
//!
//! * `dex-input`: a `.dex` file (also a v41 container), or every zip entry that starts with the
//!   dex magic or is named `*.dex` (this covers `assets/*.dex` and extension-less dex files of
//!   an APK); archives inside archives are not searched for dex files,
//! * `java-input`: `.class` files and `.jar`/`.zip`/`.apk` archives; archives nested in archives
//!   (`assets/res.zip` containing a `.jar` ...) are unpacked recursively and every entry that
//!   starts with the class magic (or is named `*.class`) is loaded,
//! * `xapk-input`, `apks-input`, `apkm-input`: the contained APKs of a split package are loaded
//!   by the dex input.
//!
//! The reference runs all plugins on every input, so an APK is scanned by the dex input and by
//! the java input. The loaded classes are merged by [`InputSet::class_sources`] and
//! [`InputSet::deduplicated`], which reproduce the duplicate handling of the reference root.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use jadx_dex_input::{DexFile, LoadOptions as DexLoadOptions};
use jadx_java_input::JavaClass;

use crate::container::{OpenOptions, PackageContainer, PackageKind};
use crate::dup::{select_from_duplicates, ClassSource, Origin};
use crate::error::Result;

/// Options of [`load_inputs`].
#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// Verify the Adler-32 checksum of every dex (reference default: on).
    pub verify_checksum: bool,
    /// Load `.class` files from archives (the reference `java-input` plugin).
    pub java_classes: bool,
    /// Strict package validation, see [`OpenOptions::strict`].
    pub strict_containers: bool,
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions {
            verify_checksum: true,
            java_classes: true,
            strict_containers: false,
        }
    }
}

/// Everything loaded from a set of input paths.
#[derive(Debug, Default)]
pub struct InputSet {
    pub dex_files: Vec<DexFile>,
    pub java_classes: Vec<JavaClass>,
    /// Opened package containers (`.xapk`, `.apks`, `.apkm`) in input order.
    pub containers: Vec<Arc<PackageContainer>>,
    /// Non-fatal problems; bad entries are skipped like in the reference.
    pub warnings: Vec<String>,
}

fn raw_name(desc: &str) -> String {
    let inner = desc
        .strip_prefix('L')
        .and_then(|d| d.strip_suffix(';'))
        .unwrap_or(desc);
    inner.replace('/', ".")
}

impl InputSet {
    /// One record per loaded class in load order: dex classes first, then JVM classes.
    pub fn class_sources(&self) -> Vec<ClassSource> {
        let mut out = Vec::new();
        for (dex_idx, dex) in self.dex_files.iter().enumerate() {
            for def in dex.class_defs().flatten() {
                if let Ok(desc) = dex.type_descriptor(def.class_idx) {
                    out.push(ClassSource {
                        raw_name: raw_name(&desc),
                        source: dex.name().to_owned(),
                        origin: Origin::Dex(dex_idx),
                    });
                }
            }
        }
        for (idx, cls) in self.java_classes.iter().enumerate() {
            if let Ok(desc) = cls.type_desc() {
                out.push(ClassSource {
                    raw_name: raw_name(&desc),
                    source: cls.source().to_owned(),
                    origin: Origin::Java(idx),
                });
            }
        }
        out
    }

    /// Class records after the duplicate selection of the reference, sorted by raw name.
    /// The second value lists a warning per name that was defined more than once.
    pub fn deduplicated(&self) -> (Vec<ClassSource>, Vec<String>) {
        select_from_duplicates(self.class_sources())
    }
}

/// Load the code inputs of all `paths`.
pub fn load_inputs(paths: &[PathBuf], opts: &LoadOptions) -> InputSet {
    let mut set = InputSet::default();
    let dex_opts = DexLoadOptions {
        verify_checksum: opts.verify_checksum,
    };
    // dex input
    for path in paths {
        match jadx_dex_input::load_path(path, &dex_opts) {
            Ok(res) => {
                set.dex_files.extend(res.dex_files);
                set.warnings.extend(res.warnings);
            }
            Err(e) => set
                .warnings
                .push(format!("File open error: {}: {e}", path.display())),
        }
    }
    // java input
    if opts.java_classes {
        for path in paths {
            match jadx_java_input::load_path(path) {
                Ok(res) => {
                    set.java_classes.extend(res.classes);
                    set.warnings.extend(res.warnings);
                }
                Err(e) => set
                    .warnings
                    .push(format!("File open error: {}: {e}", path.display())),
            }
        }
    }
    // package containers
    for path in paths {
        if !PackageKind::from_path(path).is_split_container() {
            continue;
        }
        let open = OpenOptions {
            strict: opts.strict_containers,
        };
        let container = match PackageContainer::open(path, &open) {
            Ok(c) => c,
            Err(e) => {
                set.warnings
                    .push(format!("Failed to load package: {}: {e}", path.display()));
                continue;
            }
        };
        set.warnings.extend(container.warnings().iter().cloned());
        for apk in container.apk_files() {
            match jadx_dex_input::load_path(apk, &dex_opts) {
                Ok(res) => {
                    set.dex_files.extend(res.dex_files);
                    set.warnings.extend(res.warnings);
                }
                Err(e) => set
                    .warnings
                    .push(format!("File open error: {}: {e}", apk.display())),
            }
        }
        set.containers.push(container);
    }
    set
}

/// Load one input path.
pub fn load_input(path: &Path, opts: &LoadOptions) -> Result<InputSet> {
    std::fs::metadata(path)?;
    Ok(load_inputs(&[path.to_path_buf()], opts))
}
