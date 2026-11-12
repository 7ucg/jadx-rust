//! Regression test: the code strings of the corpus instructions must equal the output of the
//! reference code generator (`corpus.code.expected.txt`, produced by
//! `tests/oracle/codegen_oracle.sh`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/oracle/corpus")
}

/// Method blocks keyed by method id (class iteration order is irrelevant).
fn by_method(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut cur: Option<String> = None;
    for line in text.lines() {
        if line.starts_with("M ") {
            cur = Some(line.to_string());
            out.entry(line.to_string()).or_default();
        } else if let (Some(m), true) = (&cur, line.starts_with("S ") || line.starts_with("W ")) {
            out.get_mut(m).unwrap().push(line.to_string());
        }
    }
    out
}

#[test]
fn corpus_code_matches_reference() {
    let out = Command::new(env!("CARGO_BIN_EXE_jadx-rs"))
        .arg("insn-code")
        .arg(corpus_dir().join("corpus.dex"))
        .env("JADX_RS_STAGE", "11")
        .output()
        .expect("run jadx-rs");
    assert!(out.status.success());
    let actual = by_method(&String::from_utf8(out.stdout).unwrap());
    let expected =
        by_method(&std::fs::read_to_string(corpus_dir().join("corpus.code.expected.txt")).unwrap());
    assert!(!expected.is_empty());
    assert_eq!(actual, expected);
}
