//! Loader behaviour on synthetic archives: dex files anywhere in an APK, JVM classes in nested
//! archives, split packages and duplicate selection.

use std::io::{Cursor, Write};
use std::path::PathBuf;

use jadx_package::{load_inputs, LoadOptions, Origin};

const CORPUS_DEX: &[u8] = include_bytes!("../../../tests/oracle/corpus/corpus.dex");

fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// Smallest valid class file `public class <name> extends java.lang.Object`.
fn class_bytes(name: &str) -> Vec<u8> {
    let mut b = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 52, 0, 5];
    b.push(1);
    b.extend((name.len() as u16).to_be_bytes());
    b.extend(name.as_bytes());
    b.extend([7, 0, 1]);
    b.push(1);
    b.extend(16u16.to_be_bytes());
    b.extend(b"java/lang/Object");
    b.extend([7, 0, 3]);
    b.extend([0, 0x21, 0, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0]);
    b
}

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_tmp(tag: &str, ext: &str, bytes: &[u8]) -> Tmp {
    let path = std::env::temp_dir().join(format!(
        "jadx-package-test-{}-{tag}.{ext}",
        std::process::id()
    ));
    std::fs::write(&path, bytes).unwrap();
    Tmp(path)
}

fn names(set: &jadx_package::InputSet) -> Vec<String> {
    set.deduplicated()
        .0
        .into_iter()
        .map(|c| c.raw_name)
        .collect()
}

#[test]
fn loads_dex_from_assets_and_classes_from_nested_archives() {
    let jar = zip_bytes(&[("p/Inner.class", &class_bytes("p/Inner"))]);
    let res_zip = zip_bytes(&[("libs/lib.jar", &jar)]);
    let apk = zip_bytes(&[
        ("classes.dex", CORPUS_DEX),
        // extension-less dex file, found by its magic
        ("assets/android_dex", CORPUS_DEX),
        ("assets/res.zip", &res_zip),
        ("assets/Plain.bin", &class_bytes("p/Plain")),
    ]);
    let t = write_tmp("nested", "apk", &apk);
    let opts = LoadOptions {
        verify_checksum: false,
        ..LoadOptions::default()
    };
    let set = load_inputs(std::slice::from_ref(&t.0), &opts);
    assert_eq!(set.dex_files.len(), 2);
    assert_eq!(set.java_classes.len(), 2);
    let (kept, warnings) = set.deduplicated();
    // the three corpus classes exist twice and are reported once each
    assert_eq!(warnings.len(), 3);
    let n: Vec<_> = kept.iter().map(|c| c.raw_name.as_str()).collect();
    assert_eq!(
        n,
        [
            "corpus.Annot",
            "corpus.Finally",
            "corpus.Ops",
            "p.Inner",
            "p.Plain"
        ]
    );
    // the duplicate selection prefers classes.dex over the asset dex
    let annot = kept.iter().find(|c| c.raw_name == "corpus.Annot").unwrap();
    assert_eq!(annot.source, "classes.dex");
    assert!(matches!(annot.origin, Origin::Dex(0)));
    let inner = kept.iter().find(|c| c.raw_name == "p.Inner").unwrap();
    assert!(inner
        .source
        .ends_with(":assets/res.zip:libs/lib.jar:p/Inner.class"));

    let no_java = LoadOptions {
        java_classes: false,
        ..opts
    };
    assert_eq!(
        names(&load_inputs(std::slice::from_ref(&t.0), &no_java)).len(),
        3
    );
}

#[test]
fn loads_every_apk_of_a_split_package() {
    let base = zip_bytes(&[
        (
            "AndroidManifest.xml",
            br#"<manifest package="a.b" android:versionCode="1"/>"#,
        ),
        ("classes.dex", CORPUS_DEX),
    ]);
    let feature = zip_bytes(&[
        (
            "AndroidManifest.xml",
            br#"<manifest package="a.b" android:versionCode="1" split="f"/>"#,
        ),
        ("classes.dex", CORPUS_DEX),
    ]);
    let manifest = br#"{"xapk_version":2,"split_apks":[{"file":"base.apk"},{"file":"f.apk"}]}"#;
    let xapk = zip_bytes(&[
        ("manifest.json", manifest),
        ("base.apk", &base),
        ("f.apk", &feature),
    ]);
    let t = write_tmp("split", "xapk", &xapk);
    let opts = LoadOptions {
        verify_checksum: false,
        ..LoadOptions::default()
    };
    let set = load_inputs(std::slice::from_ref(&t.0), &opts);
    assert_eq!(set.containers.len(), 1);
    assert_eq!(set.dex_files.len(), 2);
    assert!(set.warnings.is_empty(), "{:?}", set.warnings);
    // nested APKs are scanned for classes by the java input as well (none here)
    assert_eq!(set.java_classes.len(), 0);
    assert_eq!(names(&set).len(), 3);
    let split_paths: Vec<_> = set.containers[0]
        .entries()
        .iter()
        .filter(|e| e.path.starts_with("splits/f/"))
        .map(|e| e.path.clone())
        .collect();
    assert_eq!(
        split_paths,
        ["splits/f/AndroidManifest.xml", "splits/f/classes.dex"]
    );
}

#[test]
fn bad_inputs_are_warnings_not_failures() {
    let t = write_tmp("bad", "apk", b"PK\x03\x04 definitely not a zip");
    let set = load_inputs(
        &[t.0.clone(), PathBuf::from("/nonexistent/file.apk")],
        &LoadOptions::default(),
    );
    assert!(set.dex_files.is_empty());
    assert!(!set.warnings.is_empty());
    let t = write_tmp("bad2", "xapk", &zip_bytes(&[("x.txt", b"y")]));
    let set = load_inputs(std::slice::from_ref(&t.0), &LoadOptions::default());
    assert!(set.containers.is_empty());
    assert!(set
        .warnings
        .iter()
        .any(|w| w.contains("Failed to load package")));
}
