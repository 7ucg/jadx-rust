//! Regression test: smali output for the hand-written corpus must equal the reference output
//! (`corpus.expected.txt`, produced by the reference disassembler via `tests/oracle/corpus_oracle.sh`).

use std::path::PathBuf;

use jadx_dex_input::{load_path, LoadOptions};

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/oracle/corpus")
}

#[test]
fn smali_matches_reference() {
    let res = load_path(&corpus_dir().join("corpus.dex"), &LoadOptions::default()).unwrap();
    assert_eq!(res.dex_files.len(), 1);
    let dex = &res.dex_files[0];
    let mut out = String::new();
    for i in 0..dex.class_count() {
        out.push_str(&dex.class_smali(i).unwrap());
        out.push_str("\n#---end-class---\n");
    }
    let expected = std::fs::read_to_string(corpus_dir().join("corpus.expected.txt")).unwrap();
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        expected.lines().collect::<Vec<_>>()
    );
}

#[test]
fn all_opcodes_are_exercised() {
    use std::collections::HashSet;
    let res = load_path(&corpus_dir().join("corpus.dex"), &LoadOptions::default()).unwrap();
    let dex = &res.dex_files[0];
    let mut seen: HashSet<&'static str> = HashSet::new();
    for def in dex.class_defs() {
        let def = def.unwrap();
        let Some(data) = dex.class_data(&def).unwrap() else {
            continue;
        };
        for m in data.direct_methods.iter().chain(&data.virtual_methods) {
            if m.code_off != 0 {
                let code = dex.code_item(m.code_off).unwrap();
                for insn in jadx_dex_input::insns::decode_all(&code.insns).unwrap() {
                    seen.insert(insn.info.name);
                }
            }
        }
    }
    // opcodes that real-world APKs rarely or never contain
    for name in [
        "const-method-handle",
        "const-method-type",
        "goto/32",
        "invoke-custom",
        "invoke-custom/range",
        "invoke-polymorphic",
        "invoke-polymorphic/range",
        "move/16",
        "move-wide/16",
        "sget-short",
        "sput-short",
        "packed-switch-payload",
        "sparse-switch-payload",
        "array-payload",
    ] {
        assert!(seen.contains(name), "corpus does not exercise {name}");
    }
}

/// Corrupted input must produce errors or odd text, never a panic.
#[test]
fn corrupted_corpus_never_panics() {
    use jadx_dex_input::load_bytes;
    let base = std::fs::read(corpus_dir().join("corpus.dex")).unwrap();
    let opts = LoadOptions {
        verify_checksum: false,
    };
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..30_000 {
        let mut b = base.clone();
        for _ in 0..1 + next() % 6 {
            let pos = (next() as usize) % b.len();
            b[pos] = next() as u8;
        }
        let Ok(res) = load_bytes("fuzz.dex", b, &opts, false) else {
            continue;
        };
        for dex in &res.dex_files {
            for i in 0..dex.class_count().min(16) {
                let _ = dex.class_smali(i);
            }
        }
    }
}
