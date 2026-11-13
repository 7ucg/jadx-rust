//! Gradle project export (`jadx.core.export.*` / `jadx.core.export.gen.*`): writes an
//! Android Studio / Gradle project skeleton (`build.gradle`, `settings.gradle`,
//! `gradle.properties`) around the saved sources and resources, so the decompiled output can be
//! opened directly as a project.
//!
//! Templates are the reference's own (`jadx-core/src/main/resources/export/**`), embedded at
//! compile time; the simple `{{var}}` / `{{!!var}}` substitution syntax matches
//! `jadx.core.export.TemplateFile`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::args::{ExportGradleType, JadxArgs};
use crate::paths::prepare_file;
use crate::resources::{ResourceFile, ResourceType};

const ANDROID_BUILD_GRADLE: &str = include_str!("../templates/export/android/build.gradle.tmpl");
const ANDROID_APP_BUILD_GRADLE: &str =
    include_str!("../templates/export/android/app.build.gradle.tmpl");
const ANDROID_LIB_BUILD_GRADLE: &str =
    include_str!("../templates/export/android/lib.build.gradle.tmpl");
const ANDROID_SETTINGS_GRADLE: &str =
    include_str!("../templates/export/android/settings.gradle.tmpl");
const JAVA_BUILD_GRADLE_KTS: &str = include_str!("../templates/export/java/build.gradle.kts.tmpl");
const JAVA_SETTINGS_GRADLE_KTS: &str =
    include_str!("../templates/export/java/settings.gradle.kts.tmpl");

/// Source and resource output directories chosen for a save run (`jadx.core.export.OutDirs`).
#[derive(Debug, Clone)]
pub struct OutDirs {
    pub src: PathBuf,
    pub res: PathBuf,
}

impl OutDirs {
    pub fn make_dirs(&self) -> io::Result<()> {
        fs::create_dir_all(&self.src)?;
        fs::create_dir_all(&self.res)?;
        Ok(())
    }
}

/// Parsed `AndroidManifest.xml` attributes used by the generated `build.gradle`
/// (`jadx.core.utils.android.ApplicationParams`).
#[derive(Debug, Clone, Default)]
pub struct ApplicationParams {
    pub application_label: Option<String>,
    pub min_sdk_version: Option<i64>,
    pub target_sdk_version: Option<i64>,
    pub compile_sdk_version: Option<i64>,
    pub version_code: Option<i64>,
    pub version_name: Option<String>,
}

/// Flags collected while decoding resources and code that affect the generated Gradle files
/// (`jadx.core.export.GradleInfoStorage`). Detection of these flags is not ported yet (see
/// `NonFinalResIdsVisitor`/`BinaryXMLParser`/`UsageInfo` in the reference), so they default to
/// `false`: the corresponding Gradle options (`android.nonFinalResIds`, the support library
/// vector drawable flags, the Apache HTTP legacy library) are simply left out of the generated
/// project, which Gradle itself does not require unless the decompiled code actually needs them.
#[derive(Debug, Clone, Copy, Default)]
pub struct GradleInfoStorage {
    pub vector_path_data: bool,
    pub vector_fill_type: bool,
    pub use_apache_http_legacy: bool,
    pub non_final_res_ids: bool,
}

/// `jadx.core.export.ExportGradle.detectExportType`: Android app if the archive has a manifest
/// and a resource table, Android library if it has a manifest and a `classes.jar` (AAR layout),
/// otherwise a plain Java project.
pub fn detect_export_type(resources: &[ResourceFile]) -> ExportGradleType {
    let has_manifest = resources
        .iter()
        .any(|r| r.resource_type() == ResourceType::Manifest);
    if !has_manifest {
        return ExportGradleType::SimpleJava;
    }
    if resources
        .iter()
        .any(|r| r.original_name().ends_with("classes.jar"))
    {
        return ExportGradleType::AndroidLibrary;
    }
    if resources
        .iter()
        .any(|r| r.resource_type() == ResourceType::Arsc)
    {
        return ExportGradleType::AndroidApp;
    }
    ExportGradleType::SimpleJava
}

/// Resolve the effective export type: `args`' explicit choice wins unless it is `Auto` (or
/// equal to the detected type), matching `ExportGradle.getExportGradleType`.
pub fn effective_export_type(
    args_type: Option<ExportGradleType>,
    resources: &[ResourceFile],
) -> ExportGradleType {
    let detected = detect_export_type(resources);
    match args_type {
        None | Some(ExportGradleType::Auto) => detected,
        Some(t) if t == detected => detected,
        Some(t) => t,
    }
}

/// `jadx.core.export.gen.GradleGeneratorTools.guessProjectName`.
pub fn guess_project_name(args: &JadxArgs) -> String {
    if args.input_files.len() == 1 {
        let path = &args.input_files[0];
        return path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| "PROJECT_NAME".to_string());
    }
    "PROJECT_NAME".to_string()
}

/// `TemplateFile.add(name, Object)` does `String.valueOf(value)`; a `null` `Integer`/`String`
/// (an attribute the reference does not run through `Utils.getOrElse`, e.g.
/// `compileSdkVersion`/`versionCode`/`versionName`) therefore renders as the literal `"null"`.
fn java_null_i64(v: Option<i64>) -> String {
    match v {
        Some(n) => n.to_string(),
        None => "null".to_string(),
    }
}

fn java_null_str(v: Option<&str>) -> String {
    v.unwrap_or("null").to_string()
}

/// Minimal escaping of untrusted manifest/package data embedded in a generated build script.
/// The reference delegates to `IJadxSecurity.sanitizeString`, which this port does not have yet;
/// this keeps a value from breaking out of its quoted literal or injecting a Kotlin/Groovy
/// string template (`${...}`/`"""`).
pub fn sanitize_gradle_groovy(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('"', "\\\"")
        .replace('\n', " ")
        .replace('\r', "")
}

pub fn sanitize_gradle_kotlin(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('\n', " ")
        .replace('\r', "")
}

/// Simple `{{var}}` template engine (`jadx.core.export.TemplateFile`): `{{!!var}}` inserts the
/// raw value, `{{var}}` sanitizes it first.
fn render_template(
    template: &str,
    vars: &[(&str, &str)],
    sanitize: impl Fn(&str) -> String,
) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = template[i + 2..].find("}}") {
                let name = &template[i + 2..i + 2 + end];
                let (raw, var_name) = match name.strip_prefix("!!") {
                    Some(rest) => (true, rest),
                    None => (false, name),
                };
                match vars.iter().find(|(n, _)| *n == var_name) {
                    Some((_, value)) => {
                        if raw {
                            out.push_str(value);
                        } else {
                            out.push_str(&sanitize(value));
                        }
                    }
                    None => {
                        // Unknown variable: keep the reference's behaviour of failing loudly
                        // impossible without panicking an API consumer; leave the placeholder
                        // untouched so the problem is visible in the generated file.
                        out.push_str("{{");
                        out.push_str(name);
                        out.push_str("}}");
                    }
                }
                i += 2 + end + 2;
                continue;
            }
        }
        // copy one UTF-8 char
        let ch_len = template[i..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(1);
        out.push_str(&template[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn save(path: &Path, content: &str) -> io::Result<()> {
    prepare_file(path)?;
    fs::write(path, content)
}

/// Options for [`export_gradle_project`].
pub struct GradleExportInput<'a> {
    pub args: &'a JadxArgs,
    pub project_dir: &'a Path,
    pub resources: &'a [ResourceFile],
    pub app_package: Option<&'a str>,
    pub gradle_info: GradleInfoStorage,
    pub application_params: ApplicationParams,
}

/// `jadx.core.export.ExportGradle.init`: picks the source/resource output directories for the
/// chosen export type, without writing any Gradle file yet (the caller still has to save the
/// sources and resources below them).
pub fn init_out_dirs(export_type: ExportGradleType, project_dir: &Path) -> OutDirs {
    match export_type {
        ExportGradleType::AndroidApp | ExportGradleType::AndroidLibrary => {
            let module_dir = if export_type == ExportGradleType::AndroidApp {
                "app"
            } else {
                "lib"
            };
            let base = project_dir.join(module_dir);
            OutDirs {
                src: base.join("src/main/java"),
                res: base.join("src/main"),
            }
        }
        ExportGradleType::SimpleJava | ExportGradleType::Auto => {
            let base = project_dir.join("app");
            OutDirs {
                src: base.join("src/main/java"),
                res: base.join("src/main/resources"),
            }
        }
    }
}

/// `jadx.core.export.ExportGradle.generateGradleFiles`: writes the Gradle project files. Call
/// after the sources and resources have been saved below [`init_out_dirs`]'s directories.
pub fn generate_gradle_files(input: &GradleExportInput<'_>) -> io::Result<()> {
    let export_type = effective_export_type(input.args.export_gradle_type, input.resources);
    match export_type {
        ExportGradleType::AndroidApp => generate_android(input, true),
        ExportGradleType::AndroidLibrary => generate_android(input, false),
        ExportGradleType::SimpleJava | ExportGradleType::Auto => generate_simple_java(input),
    }
}

fn generate_android(input: &GradleExportInput<'_>, export_app: bool) -> io::Result<()> {
    let project_dir = input.project_dir;
    let module_dir = if export_app { "app" } else { "lib" };
    let base_dir = project_dir.join(module_dir);
    let params = &input.application_params;

    // build.gradle (project root)
    save(&project_dir.join("build.gradle"), ANDROID_BUILD_GRADLE)?;

    // settings.gradle
    let project_name = params
        .application_label
        .clone()
        .unwrap_or_else(|| guess_project_name(input.args));
    let settings = render_template(
        ANDROID_SETTINGS_GRADLE,
        &[
            ("projectName", project_name.as_str()),
            ("mainModuleName", module_dir),
        ],
        sanitize_gradle_groovy,
    );
    save(&project_dir.join("settings.gradle"), &settings)?;

    // module build.gradle
    let min_sdk = params.min_sdk_version.unwrap_or(0);
    let additional_options = gen_additional_android_plugin_options(input.gradle_info, min_sdk);
    if export_app {
        let app_package = input.app_package.unwrap_or("UNKNOWN");
        let build = render_template(
            ANDROID_APP_BUILD_GRADLE,
            &[
                ("applicationId", app_package),
                ("minSdkVersion", &min_sdk.to_string()),
                (
                    "compileSdkVersion",
                    &java_null_i64(params.compile_sdk_version),
                ),
                (
                    "targetSdkVersion",
                    &java_null_i64(params.target_sdk_version),
                ),
                ("versionCode", &java_null_i64(params.version_code)),
                (
                    "versionName",
                    &java_null_str(params.version_name.as_deref()),
                ),
                ("additionalOptions", &additional_options),
            ],
            sanitize_gradle_groovy,
        );
        save(&base_dir.join("build.gradle"), &build)?;
    } else {
        let pkg = input.app_package.unwrap_or("UNKNOWN");
        let build = render_template(
            ANDROID_LIB_BUILD_GRADLE,
            &[
                ("packageId", pkg),
                ("minSdkVersion", &min_sdk.to_string()),
                (
                    "compileSdkVersion",
                    &java_null_i64(params.compile_sdk_version),
                ),
                ("additionalOptions", &additional_options),
            ],
            sanitize_gradle_groovy,
        );
        save(&base_dir.join("build.gradle"), &build)?;
    }

    // gradle.properties
    if input.gradle_info.non_final_res_ids {
        save(
            &project_dir.join("gradle.properties"),
            "android.nonFinalResIds=false",
        )?;
    }
    Ok(())
}

fn gen_additional_android_plugin_options(info: GradleInfoStorage, min_sdk: i64) -> String {
    let mut options: Vec<&str> = Vec::new();
    if (info.vector_path_data && min_sdk < 21) || (info.vector_fill_type && min_sdk < 24) {
        options.push("vectorDrawables.useSupportLibrary = true");
    }
    if info.use_apache_http_legacy {
        options.push("useLibrary 'org.apache.http.legacy'");
    }
    let mut out = String::new();
    for opt in options {
        out.push_str("        ");
        out.push_str(opt);
        out.push('\n');
    }
    out
}

fn generate_simple_java(input: &GradleExportInput<'_>) -> io::Result<()> {
    let project_dir = input.project_dir;
    let app_dir = project_dir.join("app");
    let project_name = guess_project_name(input.args);
    let settings = render_template(
        JAVA_SETTINGS_GRADLE_KTS,
        &[("projectName", project_name.as_str())],
        sanitize_gradle_kotlin,
    );
    save(&project_dir.join("settings.gradle.kts"), &settings)?;
    save(&app_dir.join("build.gradle.kts"), JAVA_BUILD_GRADLE_KTS)?;
    Ok(())
}

// ---------------------------------------------------------------------- AndroidManifest.xml

/// Minimal attribute reader for the decoded (text) `AndroidManifest.xml`
/// (`jadx.core.utils.android.AndroidManifestParser`), without pulling in a full XML parser:
/// the manifest produced by this port's resource decoder always prints one `attr="value"` per
/// attribute with plain double quotes, so a small tag/attribute scan is enough.
pub struct ManifestAttrs<'a> {
    xml: &'a str,
}

impl<'a> ManifestAttrs<'a> {
    pub fn new(xml: &'a str) -> Self {
        ManifestAttrs { xml }
    }

    /// First occurrence of `<tag ...>` (self closing or not, attributes possibly on their own
    /// indented lines as this port's pretty printer emits them), its attribute section only.
    fn find_tag(&self, tag: &str) -> Option<&'a str> {
        let open = format!("<{tag}");
        let mut search_from = 0;
        while let Some(rel) = self.xml[search_from..].find(open.as_str()) {
            let start = search_from + rel;
            let after = start + open.len();
            // the tag name must end here: next char is whitespace, '>' or '/', not part of a
            // longer tag name (e.g. "<uses-sdk" must not match "<uses-sdk-extension").
            let next_ch = self.xml[after..].chars().next();
            let is_boundary =
                matches!(next_ch, Some(c) if c.is_whitespace() || c == '>' || c == '/');
            if is_boundary {
                let rest = &self.xml[start..];
                let end = rest.find('>')?;
                return Some(&rest[..end]);
            }
            search_from = after;
        }
        None
    }

    fn attr(&self, tag_text: &str, attr: &str) -> Option<String> {
        let needle = format!("{attr}=\"");
        let pos = tag_text.find(&needle)?;
        let rest = &tag_text[pos + needle.len()..];
        let end = rest.find('"')?;
        let value = &rest[..end];
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    }

    pub fn uses_sdk_attr(&self, attr: &str) -> Option<String> {
        self.find_tag("uses-sdk").and_then(|t| self.attr(t, attr))
    }

    pub fn manifest_attr(&self, attr: &str) -> Option<String> {
        self.find_tag("manifest").and_then(|t| self.attr(t, attr))
    }

    pub fn application_attr(&self, attr: &str) -> Option<String> {
        self.find_tag("application")
            .and_then(|t| self.attr(t, attr))
    }
}

fn parse_int(s: &str) -> Option<i64> {
    // `Utils.safeParseInteger`: plain base-10, ignores anything that does not parse.
    s.trim().parse::<i64>().ok()
}

/// Resolve `android:label="@string/name"` against a decoded `strings.xml`
/// (`AndroidManifestParser.getApplicationLabel`).
fn resolve_string_ref(strings_xml: Option<&str>, value: &str) -> Option<String> {
    let name = value.strip_prefix("@string/")?;
    let xml = strings_xml?;
    let needle = format!("name=\"{name}\"");
    let pos = xml.find(&needle)?;
    let after = &xml[pos..];
    let gt = after.find('>')?;
    let rest = &after[gt + 1..];
    let close = rest.find("</string>")?;
    Some(rest[..close].to_string())
}

/// `AndroidManifestParser.parse`: parse the attributes the generated `build.gradle` needs.
/// `want_app_attrs` restricts parsing to the ones used for Android libraries (just
/// `minSdkVersion`): the reference only looks up the application label and the remaining
/// version/SDK attributes when exporting an app.
pub fn parse_application_params(
    manifest_xml: Option<&str>,
    strings_xml: Option<&str>,
    want_app_attrs: bool,
) -> ApplicationParams {
    let mut params = ApplicationParams::default();
    let Some(xml) = manifest_xml else {
        return params;
    };
    let attrs = ManifestAttrs::new(xml);

    if want_app_attrs {
        let label = attrs.application_attr("android:label");
        params.application_label = match label {
            Some(v) if v.starts_with("@string") => {
                resolve_string_ref(strings_xml, &v).or(Some("UNKNOWN".to_string()))
            }
            Some(v) => Some(v),
            None => Some("UNKNOWN".to_string()),
        };
    }

    params.min_sdk_version = attrs
        .uses_sdk_attr("android:minSdkVersion")
        .and_then(|s| parse_int(&s));

    if want_app_attrs {
        let target = attrs.uses_sdk_attr("android:targetSdkVersion");
        params.target_sdk_version = match target.and_then(|s| parse_int(&s)) {
            Some(v) => Some(v),
            None => params.min_sdk_version,
        };
        let compile = attrs.uses_sdk_attr("android:compileSdkVersion");
        params.compile_sdk_version = match compile.and_then(|s| parse_int(&s)) {
            Some(v) => Some(v),
            None => params.target_sdk_version,
        };
        params.version_code = attrs
            .manifest_attr("android:versionCode")
            .and_then(|s| parse_int(&s));
        params.version_name = attrs.manifest_attr("android:versionName");
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::ResourceSource;

    #[test]
    fn template_renders_known_and_raw_vars() {
        let out = render_template(
            "a={{a}} b={{!!b}} end",
            &[("a", "x\"y"), ("b", "<raw>")],
            |s| s.replace('"', "\\\""),
        );
        assert_eq!(out, "a=x\\\"y b=<raw> end");
    }

    #[test]
    fn detects_export_type() {
        let manifest = ResourceFile::new(
            "AndroidManifest.xml",
            ResourceType::Manifest,
            ResourceSource::File(PathBuf::from("m")),
        );
        let arsc = ResourceFile::new(
            "resources.arsc",
            ResourceType::Arsc,
            ResourceSource::File(PathBuf::from("r")),
        );
        let classes_jar = ResourceFile::new(
            "classes.jar",
            ResourceType::Archive,
            ResourceSource::File(PathBuf::from("c")),
        );
        assert_eq!(detect_export_type(&[]), ExportGradleType::SimpleJava);
        assert_eq!(
            detect_export_type(&[manifest.clone(), arsc]),
            ExportGradleType::AndroidApp
        );
        assert_eq!(
            detect_export_type(&[manifest, classes_jar]),
            ExportGradleType::AndroidLibrary
        );
    }

    #[test]
    fn parses_manifest_attrs() {
        let manifest = r#"<?xml version="1.0"?>
<manifest android:versionCode="7" android:versionName="1.2.3">
    <uses-sdk android:minSdkVersion="21" android:targetSdkVersion="33" />
    <application android:label="@string/app_name">
    </application>
</manifest>
"#;
        let strings = r#"<resources>
    <string name="app_name">My App</string>
</resources>
"#;
        let params = parse_application_params(Some(manifest), Some(strings), true);
        assert_eq!(params.min_sdk_version, Some(21));
        assert_eq!(params.target_sdk_version, Some(33));
        assert_eq!(params.compile_sdk_version, Some(33));
        assert_eq!(params.version_code, Some(7));
        assert_eq!(params.version_name.as_deref(), Some("1.2.3"));
        assert_eq!(params.application_label.as_deref(), Some("My App"));
    }

    #[test]
    fn missing_target_sdk_falls_back_to_min_sdk() {
        let manifest = r#"<manifest>
    <uses-sdk android:minSdkVersion="19" />
</manifest>
"#;
        let params = parse_application_params(Some(manifest), None, true);
        assert_eq!(params.min_sdk_version, Some(19));
        assert_eq!(params.target_sdk_version, Some(19));
        assert_eq!(params.compile_sdk_version, Some(19));
    }

    #[test]
    fn out_dirs_per_export_type() {
        let base = PathBuf::from("/out");
        let app = init_out_dirs(ExportGradleType::AndroidApp, &base);
        assert_eq!(app.src, base.join("app/src/main/java"));
        assert_eq!(app.res, base.join("app/src/main"));
        let lib = init_out_dirs(ExportGradleType::AndroidLibrary, &base);
        assert_eq!(lib.src, base.join("lib/src/main/java"));
        let java = init_out_dirs(ExportGradleType::SimpleJava, &base);
        assert_eq!(java.src, base.join("app/src/main/java"));
        assert_eq!(java.res, base.join("app/src/main/resources"));
    }
}
