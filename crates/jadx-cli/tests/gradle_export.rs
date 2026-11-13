//! End to end tests of `jadx-rs --export-gradle` against the small fixtures also used by the
//! reference CLI's own export tests (`jadx-cli/src/test/java/jadx/cli/TestExport.java`):
//! `fixtures/small.apk.bin` (Android app), `fixtures/test-lib.aar` (Android library) and
//! `fixtures/HelloWorld.class` (plain Java). The expected file counts and build.gradle values
//! below were cross checked against a run of the reference `jadx` CLI on the same inputs.
//!
//! The APK fixture is tracked as `small.apk.bin` (copied to a `small.apk` path in the test's
//! own temp dir below) because the project's `.gitignore` blocks `*.apk` everywhere to keep
//! real/private decompilation subjects out of version control; the loader only looks at the
//! zip magic bytes and the archive content, not the extension, so this makes no difference to
//! what is exercised.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn tmp(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("gradle-{name}"));
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

/// Copies the `.bin`-suffixed fixture to a real `.apk` path below a sibling input directory of
/// `name` (never below the output directory itself, so it does not show up as an extra output
/// file). The loader only looks at the zip magic bytes and archive content, not the extension,
/// so this makes no difference to what the test exercises; see the module doc comment for why
/// the fixture itself is not tracked as `.apk`.
fn apk_fixture(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("gradle-{name}-input"));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("small.apk");
    std::fs::copy(fixture("small.apk.bin"), &dest).unwrap();
    dest
}

#[test]
fn gradle_export_android_app() {
    let out = tmp("app");
    let apk = apk_fixture("app");
    let o = run(&[
        s("-d"),
        out.as_os_str(),
        s("--export-gradle"),
        apk.as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));

    let names = list_files(&out);
    assert_eq!(names.len(), 15, "{names:?}");
    assert_eq!(names.iter().filter(|f| f.ends_with(".java")).count(), 2);
    assert_eq!(names.iter().filter(|f| f.ends_with(".dex")).count(), 0);
    assert!(names.contains(&"build.gradle".to_string()));
    assert!(names.contains(&"settings.gradle".to_string()));
    assert!(names.contains(&"app/build.gradle".to_string()));
    assert!(names.contains(&"app/src/main/AndroidManifest.xml".to_string()));

    let build = std::fs::read_to_string(out.join("app/build.gradle")).unwrap();
    assert!(build.contains("id 'com.android.application'"));
    assert!(build.contains("applicationId 'io.github.skylot.android.smallapp'"));
    assert!(build.contains("minSdkVersion 26"));
    assert!(build.contains("targetSdkVersion 33"));
    assert!(build.contains("compileSdkVersion 33"));
    assert!(build.contains("versionCode 1"));
    assert!(build.contains(r#"versionName "1.0""#));

    let settings = std::fs::read_to_string(out.join("settings.gradle")).unwrap();
    assert_eq!(
        settings,
        "rootProject.name = 'Small App'\n\ninclude 'app'\n"
    );
}

#[test]
fn gradle_export_android_library() {
    let out = tmp("aar");
    let o = run(&[
        s("-d"),
        out.as_os_str(),
        s("--export-gradle"),
        fixture("test-lib.aar").as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));

    let names = list_files(&out);
    assert_eq!(names.len(), 8, "{names:?}");
    assert_eq!(
        names
            .iter()
            .filter(|f| f.starts_with("lib/src/main/java/") && f.ends_with(".java"))
            .count(),
        1
    );
    assert_eq!(names.iter().filter(|f| f.ends_with(".jar")).count(), 0);
    assert!(names.contains(&"lib/build.gradle".to_string()));
    assert!(names.contains(&"settings.gradle".to_string()));

    let build = std::fs::read_to_string(out.join("lib/build.gradle")).unwrap();
    assert!(build.contains("id 'com.android.library'"));
    assert!(build.contains("minSdk 24"));
    // the reference AndroidManifestParser never requests compileSdkVersion for a library
    // export, so the template renders Java's `String.valueOf(null)`.
    assert!(build.contains("compileSdk null"));

    let settings = std::fs::read_to_string(out.join("settings.gradle")).unwrap();
    assert_eq!(settings, "rootProject.name = 'test-lib'\n\ninclude 'lib'\n");
}

#[test]
fn gradle_export_simple_java() {
    let out = tmp("java");
    let o = run(&[
        s("-d"),
        out.as_os_str(),
        s("--export-gradle"),
        fixture("HelloWorld.class").as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));

    let names = list_files(&out);
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(
        names
            .iter()
            .filter(|f| f.ends_with(".java") && f.starts_with("app/src/main/java/"))
            .count(),
        1
    );
    assert_eq!(names.iter().filter(|f| f.ends_with(".class")).count(), 0);
    assert!(names.contains(&"settings.gradle.kts".to_string()));
    assert!(names.contains(&"app/build.gradle.kts".to_string()));
}

#[test]
fn gradle_export_explicit_type_without_matching_resources() {
    // asking for the Android app template on an input with no AndroidManifest.xml: the
    // reference still produces that template, filling the missing fields with "UNKNOWN"/null.
    let out = tmp("invalid-type");
    let o = run(&[
        s("-d"),
        out.as_os_str(),
        s("--export-gradle-type"),
        s("android-app"),
        fixture("HelloWorld.class").as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));

    let names = list_files(&out);
    assert_eq!(names.len(), 4, "{names:?}");
    assert!(names.contains(&"settings.gradle".to_string()));
    assert!(names.contains(&"build.gradle".to_string()));
    assert!(names.contains(&"app/build.gradle".to_string()));

    let build = std::fs::read_to_string(out.join("app/build.gradle")).unwrap();
    assert!(build.contains(r#"namespace "UNKNOWN""#));
    assert!(build.contains("applicationId 'UNKNOWN'"));
    assert!(build.contains("minSdkVersion 0"));
    assert!(build.contains("targetSdkVersion null"));
    assert!(build.contains("versionCode null"));
    assert!(build.contains(r#"versionName "null""#));
}
