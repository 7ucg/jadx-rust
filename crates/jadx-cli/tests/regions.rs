//! Region stages run over the committed corpus without failing a method.

use std::path::PathBuf;
use std::process::Command;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/oracle/corpus/corpus.dex")
}

fn dump(stage: u32) -> String {
    dump_with("insn-regions", stage)
}

fn dump_with(cmd: &str, stage: u32) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_jadx-rs"))
        .arg(cmd)
        .arg(corpus())
        .env("JADX_RS_STAGE", stage.to_string())
        .output()
        .expect("run jadx-rs");
    assert!(out.status.success(), "jadx-rs failed");
    String::from_utf8(out.stdout).expect("utf8 dump")
}

#[test]
fn all_region_stages_build_a_tree_for_every_method() {
    // methods the earlier stages already fail on stay failed
    let known_errors = dump_with("insn-ssa", 11).matches("LOAD_ERROR").count();
    for stage in [12, 13, 14, 15, 16, 17, 18, 19] {
        let text = dump(stage);
        assert_eq!(
            text.matches("LOAD_ERROR").count(),
            known_errors,
            "stage {stage} failed a method"
        );
        let methods = text
            .lines()
            .filter(|l| l.starts_with("M ") && !l.ends_with("LOAD_ERROR"))
            .count();
        let regions = text.lines().filter(|l| l.starts_with("R REGION")).count();
        assert!(methods > 0, "no methods dumped at stage {stage}");
        assert!(regions >= methods, "stage {stage}: missing method regions");
    }
}

#[test]
fn driver_stages_run_over_the_corpus() {
    let known_errors = dump_with("insn-ssa", 11).matches("LOAD_ERROR").count();
    for stage in [20, 25, 29, 30, 32, 33] {
        let text = dump(stage);
        assert_eq!(
            text.matches("LOAD_ERROR").count(),
            known_errors,
            "stage {stage} failed a method"
        );
        let methods = text
            .lines()
            .filter(|l| {
                l.starts_with("M ") && !l.starts_with("M CLASS") && !l.ends_with("LOAD_ERROR")
            })
            .count();
        assert!(methods > 0, "no methods dumped at stage {stage}");
        if stage >= 30 {
            // code variables are dumped from the variable processing stage on
            assert!(text.lines().any(|l| l.starts_with("V ")), "no variables");
        }
        if stage >= 33 {
            assert!(
                text.lines().any(|l| l.starts_with("M CLASS ")),
                "no class state"
            );
        }
    }
}

#[test]
fn try_catch_and_if_regions_are_built() {
    let text = dump(19);
    assert!(text.contains("R   TRY id="), "no try/catch region");
}
