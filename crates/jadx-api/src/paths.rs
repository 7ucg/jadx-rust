//! Output path handling. Every path derived from input data (class, package and resource
//! names) is validated before anything is written.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jadx_core::model::info::ClassInfo;

pub use jadx_resources::apk::is_safe_output_path;

/// Package name used for the default package.
pub const DEFAULT_PACKAGE_NAME: &str = "defpackage";

/// Join a relative path from input data to `base`. `None` if the path is not relative, has
/// other components than plain names, contains a backslash or a NUL character.
pub fn safe_join(base: &Path, rel: &str) -> Option<PathBuf> {
    if !is_safe_output_path(rel) {
        return None;
    }
    let mut out = base.to_path_buf();
    for part in rel.split('/') {
        if !part.is_empty() {
            out.push(part);
        }
    }
    Some(out)
}

/// Path of the code file of a class relative to the sources directory, without extension:
/// `pkg/path/Name`, member classes are joined to the outer class name with `_`.
pub fn class_file_path(info: &ClassInfo, rename_default_pkg: bool) -> String {
    let mut name = String::new();
    let mut chain = Vec::new();
    let mut cur = Some(info);
    while let Some(c) = cur {
        chain.push(c.short_name());
        cur = c.parent_class();
    }
    for (i, part) in chain.iter().rev().enumerate() {
        if i > 0 {
            name.push('.');
        }
        name.push_str(part);
    }
    let name = name.replace('.', "_");
    let pkg = info.package();
    let pkg = if pkg.is_empty() && rename_default_pkg {
        DEFAULT_PACKAGE_NAME
    } else {
        pkg
    };
    if pkg.is_empty() {
        return name;
    }
    format!("{}/{}", pkg.replace('.', "/"), name)
}

/// Create the parent directories of `file`.
pub fn prepare_file(file: &Path) -> io::Result<()> {
    if let Some(parent) = file.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

/// Write text followed by a line break to `file`, creating directories as needed.
pub fn save_text(file: &Path, text: &str) -> io::Result<()> {
    prepare_file(file)?;
    let mut data = Vec::with_capacity(text.len() + 1);
    data.extend_from_slice(text.as_bytes());
    data.push(b'\n');
    fs::write(file, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_rejects_traversal() {
        let base = Path::new("/out");
        assert_eq!(
            safe_join(base, "res/values/a.xml"),
            Some(PathBuf::from("/out/res/values/a.xml"))
        );
        assert_eq!(safe_join(base, "../x"), None);
        assert_eq!(safe_join(base, "a/../../x"), None);
        assert_eq!(safe_join(base, "/etc/passwd"), None);
        assert_eq!(safe_join(base, "a\\b"), None);
        assert_eq!(safe_join(base, "a\0b"), None);
        assert_eq!(safe_join(base, ""), None);
    }

    #[test]
    fn class_paths() {
        let c = ClassInfo::from_name("Lcom/a/Outer$Inner;", true).unwrap();
        assert_eq!(class_file_path(&c, true), "com/a/Outer_Inner");
        let c = ClassInfo::from_name("LTop;", true).unwrap();
        assert_eq!(class_file_path(&c, true), "defpackage/Top");
        assert_eq!(class_file_path(&c, false), "Top");
    }
}
