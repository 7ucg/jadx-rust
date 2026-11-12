//! End to end tests: the server runs in process over the small oracle corpus and is driven with
//! JSON-RPC messages (initialize, tools/list, tools/call).

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jadx_mcp::{serve, EngineLoader, SharedState};
use serde_json::{json, Value};

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/oracle/corpus/corpus.dex")
}

/// Run a whole session: initialize handshake, then `calls` as tools/call requests with ids
/// 100, 101, ... Returns the tool results in order.
fn run_calls(calls: Vec<(&str, Value)>) -> Vec<Value> {
    let mut input = String::new();
    let mut push = |v: Value| {
        input.push_str(&v.to_string());
        input.push('\n');
    };
    push(
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
    );
    push(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    for (i, (name, args)) in calls.iter().enumerate() {
        push(json!({"jsonrpc":"2.0","id":100 + i,"method":"tools/call",
            "params":{"name":name,"arguments":args}}));
    }
    let shared = SharedState::new(Arc::new(EngineLoader));
    let mut out = Vec::new();
    serve(Cursor::new(input.into_bytes()), &mut out, shared).unwrap();
    let responses: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(responses[0]["id"], json!(1));
    responses[1..].iter().map(|r| r["result"].clone()).collect()
}

/// Calls on the corpus project: open it first, return the results of `calls` only.
fn on_corpus(calls: Vec<(&str, Value)>) -> Vec<Value> {
    let mut all = vec![("open_project", json!({"path": corpus().to_string_lossy()}))];
    all.extend(calls);
    let mut results = run_calls(all);
    let open = results.remove(0);
    assert_eq!(open["isError"], json!(false), "{open}");
    results
}

fn data(result: &Value) -> &Value {
    assert_eq!(result["isError"], json!(false), "tool failed: {result}");
    &result["structuredContent"]
}

fn error_code(result: &Value) -> &str {
    assert_eq!(
        result["isError"],
        json!(true),
        "expected an error: {result}"
    );
    result["structuredContent"]["error"]["code"]
        .as_str()
        .unwrap()
}

#[test]
fn handshake_and_tool_list() {
    let input = format!(
        "{}\n{}\n{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    let shared = SharedState::new(Arc::new(EngineLoader));
    let mut out = Vec::new();
    serve(Cursor::new(input.into_bytes()), &mut out, shared).unwrap();
    let lines: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["result"]["protocolVersion"], json!("2025-06-18"));
    assert!(lines[0]["result"]["capabilities"]["tools"].is_object());
    let tools = lines[1]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in [
        "open_project",
        "get_all_classes",
        "get_class_source",
        "get_method_source",
        "get_class_disassembly",
        "get_android_manifest",
        "find_string_references",
        "find_usages",
        "get_class_hierarchy",
        "find_direct_subclasses",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}");
    }
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], json!("object"), "{tool}");
        assert!(tool["description"].as_str().is_some_and(|d| !d.is_empty()));
    }
}

#[test]
fn project_lifecycle() {
    let results = run_calls(vec![
        ("get_project_info", json!({})),
        ("open_project", json!({"path": "relative/path.apk"})),
        ("open_project", json!({"path": corpus().to_string_lossy()})),
        ("get_project_info", json!({})),
        ("close_project", json!({})),
        ("get_all_classes", json!({})),
        ("get_health", json!({})),
    ]);
    assert_eq!(error_code(&results[0]), "NO_PROJECT");
    assert_eq!(error_code(&results[1]), "INVALID_ARGUMENT");
    let info = data(&results[2]);
    assert_eq!(info["kind"], json!("dex"));
    assert_eq!(info["class_count"], json!(3));
    assert_eq!(info["capabilities"]["class_source"], json!("skeleton"));
    assert_eq!(info["capabilities"]["smali"], json!(true));
    assert_eq!(data(&results[3])["class_count"], json!(3));
    assert_eq!(data(&results[4])["closed"], json!(true));
    assert_eq!(error_code(&results[5]), "NO_PROJECT");
    assert_eq!(data(&results[6])["project_loaded"], json!(false));
}

#[test]
fn unreadable_and_unsupported_inputs_fail_cleanly() {
    let results = run_calls(vec![
        ("open_project", json!({"path": "/nonexistent/input.apk"})),
        (
            "open_project",
            json!({"path": Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml").to_string_lossy()}),
        ),
    ]);
    assert_eq!(error_code(&results[0]), "LOAD_FAILED");
    assert_eq!(error_code(&results[1]), "LOAD_FAILED");
}

#[test]
fn classes_packages_and_members() {
    let r = on_corpus(vec![
        ("get_all_classes", json!({"limit": 2})),
        ("get_all_classes", json!({"query": "fin"})),
        (
            "search_classes_by_keyword",
            json!({"query": "OPS", "kind": "class"}),
        ),
        ("list_packages", json!({})),
        ("get_class_info", json!({"class_name": "Annot"})),
        (
            "get_methods_of_class",
            json!({"class_id": "corpus.Finally"}),
        ),
        ("get_fields_of_class", json!({"class_id": "Lcorpus/Annot;"})),
        ("find_methods", json!({"name": "twoExits"})),
        (
            "find_fields",
            json!({"name": "B", "match_mode": "exact", "class_id": "corpus.Annot"}),
        ),
    ]);
    let page = data(&r[0]);
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert_eq!(page["next_offset"], json!(2));
    assert_eq!(page["total"], json!(3));
    assert_eq!(data(&r[1])["items"][0]["class_id"], json!("corpus.Finally"));
    assert_eq!(data(&r[2])["items"][0]["class_id"], json!("corpus.Ops"));
    assert_eq!(
        data(&r[3])["items"],
        json!([{"package": "corpus", "class_count": 3}])
    );
    let info = data(&r[4]);
    assert_eq!(info["class_id"], json!("corpus.Annot"));
    assert_eq!(info["kind"], json!("enum"));
    assert_eq!(info["descriptor"], json!("Lcorpus/Annot;"));
    assert_eq!(info["super_class"], json!("java.lang.Enum"));
    let methods = data(&r[5])["items"].as_array().unwrap().clone();
    assert_eq!(methods.len(), 5);
    assert!(methods
        .iter()
        .any(|m| m["method_id"] == json!("corpus.Finally.twoExits(I)I")));
    let fields = data(&r[6])["items"].as_array().unwrap().clone();
    let b = fields.iter().find(|f| f["name"] == json!("B")).unwrap();
    assert_eq!(b["field_id"], json!("corpus.Annot.B:I"));
    assert_eq!(b["constant_value"], json!("5"));
    assert_eq!(
        data(&r[7])["items"][0]["method_id"],
        json!("corpus.Finally.twoExits(I)I")
    );
    assert_eq!(
        data(&r[8])["items"][0]["field_id"],
        json!("corpus.Annot.B:I")
    );
}

#[test]
fn class_resolution_errors_carry_candidates() {
    let r = on_corpus(vec![
        ("get_class_info", json!({"class_id": "corpus.finally"})),
        ("get_class_info", json!({"class_id": "Nope"})),
        ("get_class_info", json!({})),
        (
            "get_class_info",
            json!({"class_id": "Ops", "class_name": "Annot"}),
        ),
        ("get_class_info", json!({"class_id": "Ops", "extra": 1})),
    ]);
    // different case is not an exact id, but the unique short name differs only in case
    assert_eq!(error_code(&r[0]), "SYMBOL_NOT_FOUND");
    let details = &r[0]["structuredContent"]["error"]["details"];
    assert_eq!(details["candidates"], json!(["corpus.Finally"]));
    assert_eq!(error_code(&r[1]), "SYMBOL_NOT_FOUND");
    assert_eq!(error_code(&r[2]), "INVALID_ARGUMENT");
    assert_eq!(error_code(&r[3]), "CONFLICTING_ARGUMENTS");
    let message = r[4]["structuredContent"]["error"]["message"]
        .as_str()
        .unwrap();
    assert!(message.starts_with("Unknown argument: extra; accepted arguments:"));
}

#[test]
fn source_and_disassembly() {
    let r = on_corpus(vec![
        ("get_class_source", json!({"class_id": "corpus.Finally"})),
        (
            "get_class_source",
            json!({"class_id": "corpus.Finally", "limit": 50}),
        ),
        (
            "get_class_source",
            json!({"class_id": "corpus.Finally", "format": "smali", "limit": 40}),
        ),
        ("get_class_disassembly", json!({"class_id": "Ops"})),
        (
            "get_method_source",
            json!({"method_id": "corpus.Finally.g(I)I"}),
        ),
        (
            "get_method_source",
            json!({"class_id": "Finally", "method_name": "g", "format": "smali"}),
        ),
        (
            "get_method_source",
            json!({"method_id": "Lcorpus/Finally;->h()V", "format": "smali"}),
        ),
        (
            "get_method_source",
            json!({"class_id": "Finally", "method_name": "nope"}),
        ),
    ]);
    let src = data(&r[0]);
    assert_eq!(src["source_kind"], json!("skeleton"));
    assert_eq!(src["format"], json!("java"));
    let text = src["text"].as_str().unwrap();
    assert!(text.contains("package corpus;"));
    assert!(text.contains("public class Finally {"));
    assert!(text.contains("public static int g(int p0)"));

    // paging with a stable hash
    let first = data(&r[1]);
    assert_eq!(first["returned_chars"], json!(50));
    assert_eq!(first["has_more"], json!(true));
    assert!(first["source_hash"]
        .as_str()
        .unwrap()
        .starts_with("fnv1a64:"));
    assert_eq!(first["source_hash"], src["source_hash"]);

    let smali = data(&r[2]);
    assert_eq!(smali["format"], json!("smali"));
    assert!(smali["text"].as_str().unwrap().starts_with(".class public"));

    assert!(data(&r[3])["text"]
        .as_str()
        .unwrap()
        .contains(".method public static arith(II)V"));
    let method = data(&r[4]);
    assert_eq!(method["method_id"], json!("corpus.Finally.g(I)I"));
    assert!(method["text"]
        .as_str()
        .unwrap()
        .contains("public static int g(int p0)"));
    let block = data(&r[5])["text"].as_str().unwrap();
    assert!(block.starts_with(".method public static g(I)I"));
    assert!(block.trim_end().ends_with(".end method"));
    assert!(data(&r[6])["text"]
        .as_str()
        .unwrap()
        .contains(".method public static h()V"));
    assert_eq!(error_code(&r[7]), "SYMBOL_NOT_FOUND");
}

#[test]
fn source_paging_detects_changed_documents() {
    let r = on_corpus(vec![
        (
            "get_class_source",
            json!({"class_id": "Finally", "limit": 20}),
        ),
        (
            "get_class_source",
            json!({"class_id": "Finally", "offset": 20, "source_hash": "fnv1a64:0000000000000000"}),
        ),
        (
            "get_class_source",
            json!({"class_id": "Finally", "offset": 100000}),
        ),
    ]);
    let hash = data(&r[0])["source_hash"].clone();
    assert!(hash.is_string());
    assert_eq!(error_code(&r[1]), "SOURCE_CHANGED");
    assert_eq!(error_code(&r[2]), "INVALID_ARGUMENT");
}

#[test]
fn usages_and_callees() {
    let r = on_corpus(vec![
        (
            "find_usages",
            json!({"symbol": "corpus.Finally.cleanup()V"}),
        ),
        (
            "find_usages",
            json!({"method_signature": "Lcorpus/Finally;->g(I)I"}),
        ),
        ("find_usages", json!({"symbol": "corpus.Ops.c:C"})),
        ("find_usages", json!({"symbol": "Finally"})),
        (
            "find_usages",
            json!({"symbol": "corpus.Finally.missing()V"}),
        ),
        (
            "get_method_callees",
            json!({"method_id": "corpus.Finally.twoExits(I)I"}),
        ),
        (
            "get_method_callees",
            json!({"class_id": "Finally", "method_name": "cleanup"}),
        ),
    ]);
    let callers = data(&r[0]);
    assert_eq!(callers["kind"], json!("method"));
    let ids: Vec<&str> = callers["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "corpus.Finally.twoExits(I)I",
            "corpus.Finally.withCatch(I)V"
        ]
    );
    assert_eq!(data(&r[1])["items"].as_array().unwrap().len(), 2);
    let field = data(&r[2]);
    assert_eq!(field["kind"], json!("field"));
    assert_eq!(field["items"][0]["id"], json!("corpus.Ops.fields()V"));
    assert_eq!(data(&r[3])["kind"], json!("class"));
    assert_eq!(error_code(&r[4]), "SYMBOL_NOT_FOUND");
    let callees = data(&r[5])["items"].as_array().unwrap().clone();
    assert_eq!(callees.len(), 3);
    assert!(callees.iter().all(|c| c["in_project"] == json!(true)));
    assert!(data(&r[6])["items"].as_array().unwrap().is_empty());
}

#[test]
fn hierarchy_queries_include_framework_types() {
    let r = on_corpus(vec![
        (
            "find_direct_subclasses",
            json!({"class_id": "java.lang.Enum"}),
        ),
        (
            "find_implementations",
            json!({"class_id": "java.io.Serializable"}),
        ),
        ("get_class_hierarchy", json!({"class_id": "corpus.Annot"})),
        (
            "get_class_hierarchy",
            json!({"class_id": "java.lang.Object", "depth": 1}),
        ),
        (
            "find_direct_subclasses",
            json!({"class_id": "no.such.Type"}),
        ),
    ]);
    let subs = data(&r[0]);
    assert_eq!(subs["in_project"], json!(false));
    assert_eq!(subs["items"][0]["class_id"], json!("corpus.Annot"));
    assert_eq!(subs["items"][0]["relation"], json!("extends"));
    assert_eq!(data(&r[1])["items"][0]["relation"], json!("implements"));
    let hierarchy = data(&r[2]);
    assert_eq!(
        hierarchy["ancestors"][0]["class_id"],
        json!("java.lang.Enum")
    );
    assert_eq!(
        hierarchy["interfaces"],
        json!(["java.io.Serializable", "java.lang.Comparable"])
    );
    let object = data(&r[3]);
    assert_eq!(object["subclasses"].as_array().unwrap().len(), 2);
    assert_eq!(error_code(&r[4]), "SYMBOL_NOT_FOUND");
}

#[test]
fn strings_and_string_references() {
    let r = on_corpus(vec![
        ("get_strings", json!({"query": "corpus/Annot"})),
        (
            "get_strings",
            json!({"class_id": "Ops", "query": "const", "limit": 5}),
        ),
        ("find_string_references", json!({"query": "here"})),
        (
            "find_string_references",
            json!({"value": "tab\there\nnl\rcr \"q\" 'a' \\ \u{1} \u{7f} \u{e9} \u{20ac}", "match_mode": "exact"}),
        ),
        (
            "find_string_references",
            json!({"query": "zzzz-not-present"}),
        ),
        ("search_code", json!({"query": "twoExits"})),
        (
            "search_code",
            json!({"query": "TWOEXITS", "case_sensitive": false, "package_prefix": "corpus.Finally"}),
        ),
        (
            "search_code",
            json!({"query": "twoExits", "package_prefix": "other"}),
        ),
    ]);
    assert!(!data(&r[0])["items"].as_array().unwrap().is_empty());
    assert!(data(&r[1])["items"].as_array().unwrap().len() <= 5);
    let hit = &data(&r[2])["items"][0];
    assert_eq!(hit["class_id"], json!("corpus.Ops"));
    assert_eq!(hit["method_id"], json!("corpus.Ops.consts()V"));
    assert_eq!(data(&r[3])["total"], json!(1));
    assert_eq!(data(&r[4])["items"], json!([]));
    let code = data(&r[5]);
    assert_eq!(code["source_kind"], json!("skeleton"));
    assert_eq!(code["items"][0]["class_id"], json!("corpus.Finally"));
    assert_eq!(code["items"][0]["line"], json!(8));
    assert_eq!(data(&r[6])["items"].as_array().unwrap().len(), 1);
    assert_eq!(data(&r[7])["items"], json!([]));
}

#[test]
fn input_without_resources_reports_not_found() {
    let r = on_corpus(vec![
        ("get_android_manifest", json!({})),
        ("get_manifest_summary", json!({})),
        ("get_all_resource_file_names", json!({})),
        ("get_resource_file", json!({"path": "res/layout/main.xml"})),
        ("search_resources", json!({"query": "x"})),
    ]);
    assert_eq!(error_code(&r[0]), "NOT_FOUND");
    assert_eq!(error_code(&r[1]), "NOT_FOUND");
    assert_eq!(data(&r[2])["items"], json!([]));
    assert_eq!(error_code(&r[3]), "NOT_FOUND");
    assert_eq!(data(&r[4])["items"], json!([]));
}

/// Directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("jadx-mcp-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn zip_bytes(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, content) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(content).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

const MANIFEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="corpus.app" android:versionCode="7" android:versionName="1.0">
    <uses-sdk android:minSdkVersion="21" android:targetSdkVersion="33"/>
    <uses-permission android:name="android.permission.INTERNET"/>
    <application android:label="corpus">
        <activity android:name="corpus.Finally" android:exported="true">
            <intent-filter>
                <action android:name="android.intent.action.MAIN"/>
                <category android:name="android.intent.category.LAUNCHER"/>
            </intent-filter>
        </activity>
        <service android:name="corpus.Ops" android:exported="false"/>
    </application>
</manifest>"#;

fn apk_bytes() -> Vec<u8> {
    let dex = std::fs::read(corpus()).unwrap();
    zip_bytes(&[
        ("AndroidManifest.xml", MANIFEST.as_bytes().to_vec()),
        ("classes.dex", dex),
        ("assets/readme.txt", b"hello".to_vec()),
    ])
}

fn check_app_resources(path: &Path, kind: &str) {
    let path = path.to_string_lossy().into_owned();
    let r = run_calls(vec![
        ("open_project", json!({"path": path})),
        ("get_android_manifest", json!({"limit": 60})),
        ("get_manifest_summary", json!({})),
        ("get_manifest_summary", json!({"component_type": "service"})),
        ("get_manifest_summary", json!({"exported": "true"})),
        ("get_all_resource_file_names", json!({})),
        (
            "get_resource_file",
            json!({"path": "AndroidManifest.xml", "offset": 30, "limit": 20}),
        ),
        ("search_resources", json!({"query": "LAUNCHER"})),
        ("get_class_source", json!({"class_id": "Ops"})),
    ]);
    let info = data(&r[0]);
    assert_eq!(info["kind"], json!(kind));
    assert_eq!(info["capabilities"]["resources"], json!(true));
    assert_eq!(info["class_count"], json!(3));
    let manifest = data(&r[1]);
    assert_eq!(manifest["format"], json!("xml"));
    assert_eq!(manifest["has_more"], json!(true));
    let summary = data(&r[2]);
    assert_eq!(summary["package"], json!("corpus.app"));
    assert_eq!(summary["version_code"], json!("7"));
    assert_eq!(summary["min_sdk"], json!("21"));
    assert_eq!(
        summary["uses_permissions"],
        json!(["android.permission.INTERNET"])
    );
    assert_eq!(summary["component_count"], json!(2));
    assert_eq!(summary["components"][0]["launcher"], json!(true));
    assert_eq!(data(&r[3])["components"][0]["name"], json!("corpus.Ops"));
    assert_eq!(data(&r[4])["component_count"], json!(1));
    let files = data(&r[5])["items"].as_array().unwrap().clone();
    let manifest_entry = files
        .iter()
        .find(|f| f["path"] == json!("AndroidManifest.xml"))
        .unwrap();
    assert_eq!(manifest_entry["text"], json!(true));
    assert!(files
        .iter()
        .any(|f| f["path"] == json!("assets/readme.txt") && f["text"] == json!(false)));
    assert_eq!(data(&r[6])["returned_chars"], json!(20));
    let hit = &data(&r[7])["items"][0];
    assert_eq!(hit["path"], json!("AndroidManifest.xml"));
    assert!(hit["text"].as_str().unwrap().contains("LAUNCHER"));
    assert!(data(&r[8])["text"].as_str().unwrap().contains("class Ops"));
}

#[test]
fn apk_manifest_and_resources() {
    let dir = TempDir::new("apk");
    let path = dir.0.join("app.apk");
    std::fs::write(&path, apk_bytes()).unwrap();
    check_app_resources(&path, "apk");
}

#[test]
fn xapk_bundle_uses_the_base_apk() {
    let dir = TempDir::new("xapk");
    let path = dir.0.join("bundle.xapk");
    let split = zip_bytes(&[(
        "AndroidManifest.xml",
        b"<manifest package=\"corpus.split\"/>".to_vec(),
    )]);
    let bundle = zip_bytes(&[
        ("manifest.json", b"{}".to_vec()),
        ("config.arm64.apk", split),
        ("base.apk", apk_bytes()),
    ]);
    std::fs::write(&path, bundle).unwrap();
    check_app_resources(&path, "xapk");
    // the extracted copy of the base apk must be gone once the server is dropped
    let leftovers = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(&format!("jadx-mcp-{}-", std::process::id()))
        })
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn archive_with_dex_but_no_manifest_is_a_jar_like_input() {
    let dir = TempDir::new("zip");
    let path = dir.0.join("code.jar");
    let dex = std::fs::read(corpus()).unwrap();
    std::fs::write(&path, zip_bytes(&[("classes.dex", dex)])).unwrap();
    let r = run_calls(vec![
        ("open_project", json!({"path": path.to_string_lossy()})),
        ("get_android_manifest", json!({})),
    ]);
    assert_eq!(data(&r[0])["kind"], json!("jar"));
    assert_eq!(data(&r[0])["capabilities"]["resources"], json!(false));
    assert_eq!(error_code(&r[1]), "NOT_FOUND");
}

#[test]
fn cancelled_request_is_reported() {
    let input = format!(
        "{}\n{}\n{}\n{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":5}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_health","arguments":{}}}),
    );
    let shared = SharedState::new(Arc::new(EngineLoader));
    let mut out = Vec::new();
    serve(Cursor::new(input.into_bytes()), &mut out, shared).unwrap();
    let lines: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    // The cancellation races with the request on the reader thread: it either wins (CANCELLED)
    // or the request completes normally; both leave the session consistent.
    let result = &lines[1]["result"];
    let code = result["structuredContent"]["error"]["code"].as_str();
    assert!(code.is_none() || code == Some("CANCELLED"), "{result}");
}

#[cfg(unix)]
#[test]
fn local_socket_serves_sessions_sharing_the_project() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixStream;

    let dir = TempDir::new("sock");
    let socket = dir.0.join("mcp.sock");
    let shared = SharedState::new(Arc::new(EngineLoader));
    let listener = jadx_mcp::socket::bind(&socket).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&socket).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "socket must be private to the owner");
    }
    let server = std::thread::spawn({
        let shared = shared.clone();
        move || jadx_mcp::socket::serve_listener(listener, shared, Some(2)).unwrap()
    });

    let talk = |requests: Vec<Value>| -> Vec<Value> {
        let stream = UnixStream::connect(&socket).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let mut replies = Vec::new();
        for request in requests {
            let expects_reply = request.get("id").is_some();
            writeln!(writer, "{request}").unwrap();
            if expects_reply {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                replies.push(serde_json::from_str(&line).unwrap());
            }
        }
        replies
    };
    let init = |id: u64| {
        json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}})
    };
    let initialized = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    let call = |id: u64, name: &str, args: Value| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}});

    let first = talk(vec![
        init(1),
        initialized.clone(),
        call(
            2,
            "open_project",
            json!({"path": corpus().to_string_lossy()}),
        ),
    ]);
    assert_eq!(first[1]["result"]["isError"], json!(false));
    // a second connection sees the project opened by the first one
    let second = talk(vec![
        init(1),
        initialized,
        call(2, "get_all_classes", json!({})),
    ]);
    assert_eq!(second[1]["result"]["structuredContent"]["total"], json!(3));
    server.join().unwrap();
    // a stale socket file is replaced, a live one is refused
    drop(shared);
    assert!(jadx_mcp::socket::bind(&socket).is_ok());
}

#[test]
fn binary_serves_over_stdio() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let mut child = Command::new(env!("CARGO_BIN_EXE_jadx-mcp"))
        .arg(corpus())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    fn request(
        stdin: &mut std::process::ChildStdin,
        stdout: &mut BufReader<std::process::ChildStdout>,
        value: Value,
    ) -> Value {
        writeln!(stdin, "{value}").unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    let init = request(
        &mut stdin,
        &mut stdout,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}),
    );
    assert_eq!(init["result"]["serverInfo"]["name"], json!("jadx-mcp"));
    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    // the project given on the command line is already loaded
    let classes = request(
        &mut stdin,
        &mut stdout,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"get_all_classes","arguments":{"limit":1}}}),
    );
    assert_eq!(classes["result"]["structuredContent"]["total"], json!(3));
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
