//! Loading, listing and saving of the hand written corpus through the public API.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use jadx_api::{
    ClassCodeGenerator, ClassDiagnostics, DecodedResFile, DecodedResources, GenContext, JadxArgs,
    JadxDecompiler, JadxError, Level, LogSink, ResourceDecoder, SimpleCodeInfo,
};

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/oracle/corpus/corpus.dex")
}

fn tmp(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("api-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn args_for(input: &Path, out: &Path) -> JadxArgs {
    let mut a = JadxArgs::default();
    a.add_input_file(input);
    a.set_root_dir(out);
    a
}

/// All files below `dir` with their content, sorted by relative path.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                let rel = p
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

#[derive(Default)]
struct Capture(Mutex<Vec<(Level, String)>>);

impl LogSink for Capture {
    fn log(&self, level: Level, msg: &str) {
        self.0.lock().unwrap().push((level, msg.to_string()));
    }
}

#[test]
fn lists_classes_packages_and_members() {
    let out = tmp("list");
    let mut jadx = JadxDecompiler::new(args_for(&corpus(), &out));
    jadx.load().unwrap();
    let names: Vec<&str> = jadx.classes().iter().map(|c| c.full_name()).collect();
    assert_eq!(names, ["corpus.Annot", "corpus.Finally", "corpus.Ops"]);
    let pkgs = jadx.packages();
    assert_eq!(pkgs.len(), 1);
    assert_eq!(pkgs[0].full_name(), "corpus");
    assert_eq!(pkgs[0].name(), "corpus");
    assert_eq!(pkgs[0].classes().len(), 3);
    assert!(pkgs[0].is_root() && pkgs[0].is_leaf() && !pkgs[0].is_default());

    let finally = jadx.search_class("corpus.Finally").unwrap();
    assert_eq!(finally.package(), "corpus");
    assert_eq!(finally.raw_name(), "corpus.Finally");
    assert!(!finally.is_inner());
    assert_eq!(finally.java_package().unwrap().full_name(), "corpus");
    let m = finally.search_method_by_short_id("g(I)I").unwrap();
    assert_eq!(m.name(), "g");
    assert_eq!(m.arguments().len(), 1);
    assert!(m.access_flags().is_static());
    assert!(finally.methods().len() >= 4);

    let annot = jadx.search_class("corpus.Annot").unwrap();
    let f = annot.search_field_by_short_id("B:I").unwrap();
    assert_eq!(f.name(), "B");
    assert_eq!(f.declaring_class(), annot);
    assert!(jadx.search_class("corpus.Missing").is_none());
    assert_eq!(jadx.errors_count(), 0);
}

#[test]
fn saves_class_tree_and_resource_dir() {
    let out = tmp("save");
    let mut jadx = JadxDecompiler::new(args_for(&corpus(), &out));
    jadx.load().unwrap();
    jadx.save().unwrap();
    let files = tree(&out.join("sources"));
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        [
            "corpus/Annot.java",
            "corpus/Finally.java",
            "corpus/Ops.java"
        ]
    );
    assert!(out.join("resources").is_dir());
    let text = String::from_utf8(files["corpus/Finally.java"].clone()).unwrap();
    assert!(text.starts_with("package corpus;\n\npublic class Finally {\n"));
    assert!(
        text.contains("public static int g(int arg0) {\n        /* not yet decompiled */\n    }")
    );
    assert!(text.ends_with("}\n"));
    // dex inputs are not copied as resources
    assert!(tree(&out.join("resources")).is_empty());
}

#[test]
fn output_does_not_depend_on_thread_count() {
    let mut trees = Vec::new();
    for threads in [1usize, 3, 8] {
        let out = tmp(&format!("threads{threads}"));
        let mut args = args_for(&corpus(), &out);
        args.threads_count = threads;
        let mut jadx = JadxDecompiler::new(args);
        jadx.load().unwrap();
        jadx.save().unwrap();
        trees.push(tree(&out));
    }
    assert_eq!(trees[0], trees[1]);
    assert_eq!(trees[0], trees[2]);
}

#[test]
fn skip_flags_and_filter() {
    let out = tmp("skip");
    let mut args = args_for(&corpus(), &out);
    args.skip_sources = true;
    let mut jadx = JadxDecompiler::new(args);
    jadx.load().unwrap();
    jadx.save().unwrap();
    assert!(tree(&out.join("sources")).is_empty());

    let out = tmp("filter");
    let mut args = args_for(&corpus(), &out);
    args.class_filter = Some(jadx_api::ClassFilter(Arc::new(|n| n == "corpus.Ops")));
    let mut jadx = JadxDecompiler::new(args);
    jadx.load().unwrap();
    jadx.save().unwrap();
    assert_eq!(
        tree(&out.join("sources")).keys().collect::<Vec<_>>(),
        ["corpus/Ops.java"]
    );
}

#[test]
fn default_output_dir_from_input_name() {
    let mut args = JadxArgs::default();
    args.add_input_file(corpus());
    let mut jadx = JadxDecompiler::new(args);
    jadx.set_log_sink(Arc::new(Capture::default()));
    jadx.load().unwrap();
    assert_eq!(jadx.args().out_dir.as_deref(), Some(Path::new("corpus")));
    assert_eq!(
        jadx.args().out_dir_src.as_deref(),
        Some(Path::new("corpus/sources"))
    );
}

#[test]
fn argument_validation() {
    let mut jadx = JadxDecompiler::new(JadxArgs::default());
    assert!(matches!(jadx.load(), Err(JadxError::Args(m)) if m.contains("specify input")));

    let mut args = JadxArgs::default();
    args.add_input_file("/nonexistent/input.apk");
    let mut jadx = JadxDecompiler::new(args);
    assert!(matches!(jadx.load(), Err(JadxError::Args(m)) if m.starts_with("File not found")));

    // output directory exists as a file
    let dir = tmp("validate");
    let file = dir.join("out");
    std::fs::write(&file, b"x").unwrap();
    let mut args = JadxArgs::default();
    args.add_input_file(corpus());
    args.out_dir = Some(file);
    let mut jadx = JadxDecompiler::new(args);
    assert!(matches!(jadx.load(), Err(JadxError::Args(m)) if m.contains("exists as file")));

    // saving before loading
    let jadx = JadxDecompiler::new(JadxArgs::default());
    assert!(jadx.classes().is_empty());
    assert!(jadx.save().is_err());
}

struct Plugged;

impl ClassCodeGenerator for Plugged {
    fn generate(
        &self,
        ctx: &GenContext<'_>,
        cls: usize,
        diag: &mut ClassDiagnostics,
    ) -> Result<SimpleCodeInfo, String> {
        let name = ctx.class(cls).info.full_name().to_string();
        match name.as_str() {
            "corpus.Ops" => Err("cannot generate".into()),
            "corpus.Finally" => panic!("boom"),
            _ => {
                diag.warn("unusual class");
                Ok(SimpleCodeInfo::new(format!("// {name}")))
            }
        }
    }
}

#[test]
fn generator_hook_collects_errors_and_warnings() {
    let out = tmp("plugged");
    let mut jadx = JadxDecompiler::new(args_for(&corpus(), &out));
    let log = Arc::new(Capture::default());
    jadx.set_log_sink(log.clone());
    jadx.set_class_generator(Arc::new(Plugged));
    jadx.load().unwrap();
    jadx.save().unwrap();
    assert_eq!(jadx.errors_count(), 2);
    assert_eq!(jadx.warns_count(), 1);
    let errors = jadx.errors().errors();
    assert_eq!(errors[0].0, "class corpus.Finally");
    assert!(errors[0].1[0].contains("boom"));
    assert_eq!(errors[1].0, "class corpus.Ops");

    let files = tree(&out.join("sources"));
    assert_eq!(files["corpus/Annot.java"], b"// corpus.Annot\n");
    // a failed class is still saved, as declarations with the error as comment
    let ops = String::from_utf8(files["corpus/Ops.java"].clone()).unwrap();
    assert!(ops.starts_with("/* JADX ERROR: cannot generate */\npackage corpus;"));
    jadx.print_errors_report();
    let log = log.0.lock().unwrap();
    assert!(log
        .iter()
        .any(|(l, m)| *l == Level::Error && m == "  class corpus.Ops"));
}

#[test]
fn class_code_is_available_without_saving() {
    let out = tmp("code");
    let mut jadx = JadxDecompiler::new(args_for(&corpus(), &out));
    jadx.load().unwrap();
    let cls = jadx.search_class("corpus.Annot").unwrap();
    let code = cls.code();
    assert!(code.starts_with("package corpus;\n"));
    assert!(code.contains("public static final String C = \"const\";"));
    assert!(tree(&out).is_empty() || tree(&out).keys().all(|k| !k.ends_with(".java")));
}

#[test]
fn core_generator_writes_method_bodies_and_comments() {
    let out = tmp("core-gen");
    let mut jadx = JadxDecompiler::new(args_for(&corpus(), &out));
    jadx.set_class_generator(Arc::new(jadx_api::CoreClassGenerator::new()));
    jadx.load().unwrap();
    let code = jadx.search_class("corpus.Finally").unwrap().code();
    assert!(code.starts_with("package corpus;\n\n/* JADX INFO: compiled from: Finally.smali */\n"));
    assert!(code.contains("/* JADX INFO: loaded from: "));
    assert!(code.contains("    public static int g(int i) {\n        return i;\n    }\n"));
    assert!(code.contains(
        "        try {\n            g(i);\n        } catch (RuntimeException e) {\n            h();\n        } finally {\n            cleanup();\n        }\n"
    ));
    // a method that can not be loaded is reported and replaced by a stub
    let ops = jadx.search_class("corpus.Ops").unwrap().code();
    assert!(ops.contains("/*  JADX ERROR: Method load error"));
    assert!(ops.contains(
        "throw new UnsupportedOperationException(\"Method not decompiled: corpus.Ops.newer():void\");"
    ));
    assert!(jadx.errors_count() >= 1);
    // abstract methods keep their declaration
    assert!(ops.contains("public abstract native synchronized /* bridge */ void foo(long j2, double d2, int... iArr);"));
}

fn build_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let file = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    for (name, data) in entries {
        z.start_file(*name, opts).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap();
}

#[test]
fn archive_resources_are_extracted_and_names_validated() {
    let dir = tmp("zip");
    let dex = std::fs::read(corpus()).unwrap();
    let apk = dir.join("in.apk");
    build_zip(
        &apk,
        &[
            ("classes.dex", &dex),
            ("assets/a.txt", b"hello"),
            ("res/raw/x.bin", &[0, 1, 2]),
            ("../evil.txt", b"x"),
            ("/abs.txt", b"x"),
            ("sub/../../up.txt", b"x"),
            ("back\\slash.txt", b"x"),
        ],
    );
    let out = dir.join("out");
    let mut jadx = JadxDecompiler::new(args_for(&apk, &out));
    let log = Arc::new(Capture::default());
    jadx.set_log_sink(log.clone());
    jadx.load().unwrap();
    assert_eq!(jadx.classes().len(), 3);
    let res: Vec<&str> = jadx.resources().iter().map(|r| r.original_name()).collect();
    assert_eq!(res, ["classes.dex", "assets/a.txt", "res/raw/x.bin"]);
    jadx.save().unwrap();
    let files = tree(&out.join("resources"));
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        ["assets/a.txt", "res/raw/x.bin"]
    );
    assert_eq!(files["assets/a.txt"], b"hello");
    assert_eq!(files["res/raw/x.bin"], [0, 1, 2]);
    // nothing escaped the output directory
    assert!(!dir.join("evil.txt").exists());
    assert!(!dir.join("up.txt").exists());
    let warns = log
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|(l, m)| *l == Level::Warn && m.contains("invalid name"))
        .count();
    assert_eq!(warns, 4);
}

struct FakeDecoder;

impl ResourceDecoder for FakeDecoder {
    fn decode_archive(
        &self,
        _archive: &Path,
        _args: &JadxArgs,
    ) -> Result<DecodedResources, String> {
        Ok(DecodedResources {
            files: vec![
                DecodedResFile {
                    path: "AndroidManifest.xml".into(),
                    text: "<manifest/>".into(),
                },
                DecodedResFile {
                    path: "../escape.xml".into(),
                    text: "x".into(),
                },
            ],
            warnings: vec!["Error decode res/layout/bad.xml: broken".into()],
        })
    }
}

#[test]
fn resource_decoder_hook() {
    let dir = tmp("decoder");
    let apk = dir.join("in.apk");
    build_zip(
        &apk,
        &[
            ("AndroidManifest.xml", b"binary manifest"),
            ("res/layout/ok.xml", b"binary layout"),
            ("res/layout/bad.xml", b"undecodable"),
            ("assets/data", b"d"),
        ],
    );
    let out = dir.join("out");
    let mut jadx = JadxDecompiler::new(args_for(&apk, &out));
    jadx.set_resource_decoder(Some(Arc::new(FakeDecoder)));
    jadx.load().unwrap();
    jadx.save_resources().unwrap();
    let files = tree(&out.join("resources"));
    // the decoded manifest replaces the binary one, the undecodable layout is copied unchanged
    assert_eq!(files["AndroidManifest.xml"], b"<manifest/>\n");
    assert_eq!(files["res/layout/bad.xml"], b"undecodable");
    assert!(!files.contains_key("res/layout/ok.xml"));
    assert_eq!(files["assets/data"], b"d");
    assert!(!dir.join("escape.xml").exists());
    assert!(!out.join("escape.xml").exists());

    // without a decoder everything is copied unchanged
    let out2 = dir.join("out2");
    let mut jadx = JadxDecompiler::new(args_for(&apk, &out2));
    jadx.set_resource_decoder(None);
    jadx.load().unwrap();
    jadx.save_resources().unwrap();
    assert_eq!(tree(&out2.join("resources")).len(), 4);
}
