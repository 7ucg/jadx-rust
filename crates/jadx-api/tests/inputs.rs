//! Loading of mixed code inputs through the public API: dex files in `assets/`, class files in
//! nested archives and plain class files.

use std::io::{Cursor, Write};
use std::path::PathBuf;

use jadx_api::{JadxArgs, JadxDecompiler};

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

fn tmp_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("api-inputs-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn loads_assets_dex_and_nested_class_archives() {
    let dir = tmp_dir("mixed");
    let jar = zip_bytes(&[("p/Inner.class", &class_bytes("p/Inner"))]);
    let res_zip = zip_bytes(&[("libs/lib.jar", &jar)]);
    let apk = zip_bytes(&[
        ("assets/ext.dex", CORPUS_DEX),
        ("assets/res.zip", &res_zip),
        ("classes.bin", &class_bytes("p/Plain")),
    ]);
    let input = dir.join("app.apk");
    std::fs::write(&input, apk).unwrap();

    let mut args = JadxArgs::default();
    args.add_input_file(&input);
    args.set_root_dir(dir.join("out"));
    let mut jadx = JadxDecompiler::new(args);
    jadx.load().unwrap();
    let names: Vec<&str> = jadx.classes().iter().map(|c| c.full_name()).collect();
    assert_eq!(
        names,
        [
            "corpus.Annot",
            "corpus.Finally",
            "corpus.Ops",
            "p.Inner",
            "p.Plain"
        ]
    );
    assert_eq!(jadx.errors_count(), 0);

    jadx.save().unwrap();
    let inner = std::fs::read_to_string(dir.join("out/sources/p/Inner.java")).unwrap();
    assert!(inner.contains("public class Inner {"), "{inner}");
    assert!(dir.join("out/sources/corpus/Ops.java").is_file());
}

#[test]
fn loads_a_plain_class_file() {
    let dir = tmp_dir("class");
    let input = dir.join("Plain.class");
    std::fs::write(&input, class_bytes("p/Plain")).unwrap();
    let mut args = JadxArgs::default();
    args.add_input_file(&input);
    args.set_root_dir(dir.join("out"));
    let mut jadx = JadxDecompiler::new(args);
    jadx.load().unwrap();
    assert_eq!(jadx.classes().len(), 1);
    jadx.save().unwrap();
    let code = std::fs::read_to_string(dir.join("out/sources/p/Plain.java")).unwrap();
    assert!(code.starts_with("package p;"), "{code}");
}
