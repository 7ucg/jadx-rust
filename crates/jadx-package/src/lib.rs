//! Package containers and multi-source input loading.
//!
//! * [`PackageContainer`]: opens `.apk`, `.xapk`, `.apks` and `.apkm` files and lists the files of
//!   all contained APKs under split-aware logical paths (`splits/<name>/...`, `container/...`).
//! * [`load_inputs`]: loads all code of a set of input paths the way the reference jadx input
//!   plugins do (dex files anywhere in an APK including `assets/`, JVM classes from archives
//!   nested at any depth, the APKs of split packages) into an [`InputSet`].
//! * [`select_from_duplicates`]: the duplicate class selection of the reference root.
//!
//! The container reader is adapted from RDX (Apache-2.0), see the `NOTICE` file.

pub mod container;
pub mod dup;
pub mod error;
pub mod json;
pub mod loader;
pub mod manifest;
pub mod toc;

pub use container::{
    check_zip_directory, ApkInfo, ContainerEntry, OpenOptions, PackageContainer, PackageKind,
};
pub use dup::{select_from_duplicates, ClassSource, Origin};
pub use error::{PackageError, Result};
pub use loader::{load_input, load_inputs, InputSet, LoadOptions};
pub use manifest::{parse_manifest_meta, SplitMeta};
pub use toc::{parse_toc, Toc, TocVariant};
