//! End to end tests of the `jadx-rs` binary on the hand written corpus.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/oracle/corpus/corpus.dex")
}

fn tmp(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-{name}"));
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

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn list_files(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                out.push(
                    p.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, dir, &mut v);
    v.sort();
    v
}

#[test]
fn decompiles_corpus_into_reference_layout() {
    let out = tmp("layout");
    let o = run(&[s("-d"), out.as_os_str(), corpus().as_os_str()]);
    // the corpus has a method that can not be decoded
    assert_eq!(o.status.code(), Some(3), "{}", stdout(&o));
    let text = stdout(&o);
    assert!(text.contains("INFO  - loading ..."), "{text}");
    assert!(text.contains("finished with errors"), "{text}");
    assert_eq!(
        list_files(&out),
        [
            "sources/corpus/Annot.java",
            "sources/corpus/Finally.java",
            "sources/corpus/Ops.java"
        ]
    );
    assert!(out.join("resources").is_dir());
    let annot = std::fs::read_to_string(out.join("sources/corpus/Annot.java")).unwrap();
    assert!(annot.starts_with("package corpus;\n"));
    assert!(annot.contains("public static final int B = 5;"), "{annot}");
}

#[test]
fn separate_source_and_resource_dirs_and_threads() {
    let base = tmp("dirs");
    let src = base.join("s");
    let res = base.join("r");
    let o = run(&[
        s("-ds"),
        src.as_os_str(),
        s("-dr"),
        res.as_os_str(),
        s("-j"),
        s("2"),
        s("--log-level"),
        s("error"),
        corpus().as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(3));
    assert!(!stdout(&o).contains("INFO"), "{}", stdout(&o));
    assert!(src.join("corpus/Ops.java").is_file());
    assert!(res.is_dir());
}

#[test]
fn quiet_prints_nothing_and_no_src_writes_no_classes() {
    let out = tmp("quiet");
    let o = run(&[
        s("-q"),
        s("-s"),
        s("-d"),
        out.as_os_str(),
        corpus().as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(stdout(&o), "");
    assert!(list_files(&out).is_empty());
    assert!(out.join("sources").is_dir() && out.join("resources").is_dir());
}

#[test]
fn single_class_mode() {
    let out = tmp("single");
    let o = run(&[
        s("-d"),
        out.as_os_str(),
        s("--single-class"),
        s("corpus.Finally"),
        corpus().as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    assert_eq!(list_files(&out), ["sources/corpus/Finally.java"]);

    let o = run(&[
        s("-d"),
        out.as_os_str(),
        s("--single-class"),
        s("corpus.Nope"),
        corpus().as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("Input class not found: corpus.Nope"));
}

#[test]
fn deobfuscation_renames_short_names() {
    let out = tmp("deobf");
    let o = run(&[
        s("-d"),
        out.as_os_str(),
        s("--deobf"),
        s("--deobf-min"),
        s("20"),
        corpus().as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(3), "{}", stdout(&o));
    // every corpus class, field and method name is shorter than 20 chars, so all of them are
    // renamed and the output tree moves to the generated alias names.
    let names = list_files(&out);
    assert!(
        names.iter().all(|n| n.contains("C0")),
        "expected deobfuscated class names, got {names:?}"
    );
    let any = std::fs::read_to_string(out.join(&names[0])).unwrap();
    assert!(any.contains("reason: deobfuscation"), "{any}");
}

#[test]
fn error_exit_codes() {
    let o = run(&[]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("Please specify input file"));

    let o = run(&[s("/nonexistent/in.apk")]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("File not found"));

    let o = run(&[s("--no-such-option"), corpus().as_os_str()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("Unknown option: --no-such-option"));

    let o = run(&[s("-j"), s("0"), corpus().as_os_str()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("Threads count must be positive"));

    let o = run(&[s("-d")]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("Arguments parse error"));
}

#[test]
fn no_classes_and_no_resources_fails_to_load() {
    let dir = tmp("empty");
    let input = dir.join("empty.txt");
    std::fs::write(&input, b"not a dex").unwrap();
    let o = run(&[
        s("-r"),
        s("-d"),
        dir.join("out").as_os_str(),
        input.as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stdout(&o).contains("Load failed! No classes for decompile!"));
}

#[test]
fn version_and_help() {
    let o = run(&[s("--version")]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(stdout(&o).trim(), env!("CARGO_PKG_VERSION"));

    let o = run(&[s("--help")]);
    assert_eq!(o.status.code(), Some(0));
    let text = stdout(&o);
    for opt in [
        "--output-dir",
        "--output-dir-src",
        "--output-dir-res",
        "--no-res",
        "--no-src",
        "--threads-count",
        "--show-bad-code",
        "--deobf",
        "--deobf-min",
        "--log-level",
        "--export-gradle",
        "--version",
    ] {
        assert!(text.contains(opt), "missing {opt}");
    }
}

#[test]
fn placeholders_are_accepted() {
    let out = tmp("placeholders");
    let o = run(&[
        s("--deobf"),
        s("--deobf-min"),
        s("2"),
        s("--export-gradle"),
        s("--show-bad-code"),
        s("--no-imports"),
        s("-d"),
        out.as_os_str(),
        corpus().as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(3), "{}", stdout(&o));
    assert!(stdout(&o).contains("not implemented yet"));
    assert!(out.join("sources/corpus/Annot.java").is_file());
}

#[test]
fn developer_subcommands_still_work() {
    let o = run(&[s("dex-classes"), corpus().as_os_str()]);
    assert_eq!(o.status.code(), Some(0));
    let text = stdout(&o);
    assert!(text.contains("Lcorpus/Annot;"), "{text}");

    let o = run(&[s("pre-info"), corpus().as_os_str()]);
    assert_eq!(o.status.code(), Some(0));
    assert!(stdout(&o).starts_with("C corpus.Annot"));
}
