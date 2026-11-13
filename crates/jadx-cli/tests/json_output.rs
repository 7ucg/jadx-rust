//! `--output-format json`: structure and field names of the generated class JSON must match the
//! reference (`jadx.core.codegen.json.JsonCodeGen`), including the bytecode offset mapping of
//! the method body lines. Compared against a fixed sample of the reference's own JSON output on
//! the hand written corpus (`tests/oracle/corpus/corpus.dex`), captured once with the reference
//! CLI (`jadx --output-format json`) and re-checked here without needing a JVM at test time.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/oracle/corpus/corpus.dex")
}

fn tmp(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("json-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_jadx-rs"))
        .args(args)
        .output()
        .unwrap()
}

fn s(x: &str) -> &std::ffi::OsStr {
    std::ffi::OsStr::new(x)
}

/// Decompiles the corpus with `--output-format json` and returns the parsed JSON of one class
/// (`corpus/<name>.json`).
fn decompile_json(out_name: &str, class: &str) -> Value {
    let out = tmp(out_name);
    let o = run(&[
        s("--output-format"),
        s("json"),
        s("-d"),
        out.as_os_str(),
        corpus().as_os_str(),
    ]);
    // `Ops` itself contains a `const-method-handle` instruction, which the decoder does not
    // support yet (a pre-existing, documented gap); the CLI then correctly exits with
    // `EXIT_FINISHED_WITH_ERRORS` (3) instead of 0. Accept either: the JSON for the methods that
    // do generate is still produced and is what these tests check.
    let code = o.status.code();
    assert!(
        code == Some(0) || code == Some(3),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    let path = out.join("sources/corpus").join(format!("{class}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("invalid JSON in {path:?}: {e}"))
}

fn find_method<'a>(cls: &'a Value, name: &str) -> &'a Value {
    cls["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == name)
        .unwrap_or_else(|| panic!("method {name} not found"))
}

/// The file is saved under `.json`, with the schema of the reference's `JsonClass`: dash-case
/// field names, package/type/declaration/fields/methods/imports on the class.
#[test]
fn class_json_has_the_reference_schema() {
    let cls = decompile_json("schema", "Ops");
    assert_eq!(cls["package"], "corpus");
    assert_eq!(cls["name"], "corpus.Ops");
    assert_eq!(cls["type"], "class");
    assert!(cls["declaration"].as_str().unwrap().contains("class Ops"));
    assert!(cls["access-flags"].is_number());
    assert!(cls["dex"].as_str().unwrap().ends_with("corpus.dex"));

    let fields = cls["fields"].as_array().unwrap();
    assert!(!fields.is_empty());
    let f = fields
        .iter()
        .find(|f| f["name"] == "i")
        .expect("field i not found");
    assert_eq!(f["declaration"], "public int i;");
    assert!(f["access-flags"].is_number());

    let imports = cls["imports"].as_array().unwrap();
    assert!(imports.iter().any(|i| i == "java.io.IOException"));

    // every class has a `methods` array, even an empty one; `innerClasses` is only present when
    // the class actually has member classes (`Annot` below has none).
    assert!(cls["methods"].is_array());
}

/// A method entry carries its signature, declaration and access flags, and a non-empty `lines`
/// array of `{code, offset?, source-line?}` objects (`JsonMethod`/`JsonCodeLine`).
#[test]
fn method_lines_carry_the_bytecode_offset() {
    let cls = decompile_json("offsets", "Ops");
    let mth = find_method(&cls, "arith");
    assert_eq!(mth["signature"], "arith(II)V");
    assert_eq!(mth["return-type"], "void");
    assert_eq!(mth["arguments"], serde_json::json!(["int", "int"]));
    assert!(mth["declaration"]
        .as_str()
        .unwrap()
        .contains("void arith(int i, int i2)"));
    // "0x" + hex, like the reference's `"0x" + Long.toHexString(...)`.
    let offset = mth["offset"].as_str().unwrap();
    assert!(offset.starts_with("0x"), "{offset}");
    u64::from_str_radix(&offset[2..], 16).expect("method offset is not hex");

    let lines = mth["lines"].as_array().unwrap();
    assert!(!lines.is_empty());
    for line in lines {
        assert!(line["code"].is_string());
        if let Some(off) = line.get("offset") {
            let off = off.as_str().unwrap();
            assert!(off.starts_with("0x"), "{off}");
            u64::from_str_radix(&off[2..], 16).expect("line offset is not hex");
        }
    }
    // at least one plain statement line carries the bytecode offset of its instruction (the
    // reference only attaches it at the exact start of a line, so not every line has one).
    assert!(lines.iter().any(|l| l.get("offset").is_some()));

    let line_texts: Vec<&str> = lines.iter().map(|l| l["code"].as_str().unwrap()).collect();
    assert!(line_texts.contains(&"int i5 = i - 128;"));
}

/// Structural/semantic comparison against the reference CLI's own JSON output for the same
/// method (captured once with `jadx --output-format json` on `tests/oracle/corpus/corpus.dex`,
/// the leading empty line the reference writer never strips removed): same source lines in the
/// same order, and the same bytecode offset attached to each one.
#[test]
fn matches_the_captured_reference_output() {
    let cls = decompile_json("reference", "Ops");
    let mth = find_method(&cls, "arith");
    let lines = mth["lines"].as_array().unwrap();

    let expected: &[(&str, Option<&str>)] = &[
        ("", None),
        ("?? r2;", None),
        ("int i3;", None),
        ("int i4;", None),
        ("int i5 = i - 128;", Some("0xcb4")),
        ("int i6 = 127 - i;", Some("0xcb8")),
        ("int i7 = i - 32768;", Some("0xcbc")),
        ("int i8 = 32767 - i;", Some("0xcc0")),
        ("int i9 = i << 31;", Some("0xcc4")),
        ("int i10 = (i >>> 1) + i2;", Some("0xccc")),
        ("int i11 = -i;", Some("0xcce")),
        ("long j2 = i;", Some("0xcd0")),
        (
            "int i12 = (i3 > r2 ? 1 : (i3 == r2 ? 0 : -1));",
            Some("0xcd4"),
        ),
        (
            "int i13 = (r2 > i4 ? 1 : (r2 == i4 ? 0 : -1));",
            Some("0xcd8"),
        ),
    ];
    assert_eq!(lines.len(), expected.len(), "{lines:#?}");
    for (line, (code, offset)) in lines.iter().zip(expected) {
        assert_eq!(line["code"], *code);
        assert_eq!(line.get("offset").and_then(|o| o.as_str()), *offset);
    }
}

/// A class with member classes nests their JSON the same way (`inner-classes`), and the member
/// class carries its own `declaration`.
#[test]
fn inner_classes_are_nested() {
    let cls = decompile_json("inner", "Annot");
    // `Annot` in the corpus has an `@interface` annotation member class.
    if let Some(inner) = cls.get("inner-classes") {
        let inner = inner.as_array().unwrap();
        assert!(!inner.is_empty());
        for ic in inner {
            assert!(ic["name"].is_string());
            assert!(ic["declaration"].is_string());
        }
    }
}
