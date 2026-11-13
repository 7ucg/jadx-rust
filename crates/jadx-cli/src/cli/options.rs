//! Command line options of `jadx-rs [options] <input files>`. Names and semantics follow the
//! reference command line (`JadxCLIArgs`).

use std::fmt;
use std::path::PathBuf;

use jadx_api::{
    CommentsLevel, DecompilationMode, ExportGradleType, GeneratedRenamesMappingFileMode,
    IntegerFormat, JadxArgs, OutputFormat, RenameFlag, ResourceNameSource,
    UseKotlinMethodsForVarNames, UseSourceNameAsClassNameAlias, UserRenamesMappingsMode,
};

/// Verbosity selected with `--log-level`, `-v` and `-q`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Quiet,
    Progress,
    Error,
    Warn,
    Info,
    Debug,
}

impl LogLevel {
    const NAMES: &'static [(&'static str, LogLevel)] = &[
        ("quiet", LogLevel::Quiet),
        ("progress", LogLevel::Progress),
        ("error", LogLevel::Error),
        ("warn", LogLevel::Warn),
        ("info", LogLevel::Info),
        ("debug", LogLevel::Debug),
    ];

    pub fn from_name(s: &str) -> Option<LogLevel> {
        let s = s.trim().to_ascii_lowercase();
        Self::NAMES.iter().find(|(n, _)| *n == s).map(|(_, l)| *l)
    }

    fn names() -> String {
        Self::NAMES
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// `--call-graph` output format (accepted, not implemented).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallGraphMode {
    None,
    Dot,
    Json,
}

/// Why the command line was rejected.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Malformed command line (`Arguments parse error`).
    Usage(String),
    /// Well formed but invalid value or option (`Incorrect arguments`).
    Invalid(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Usage(m) | ParseError::Invalid(m) => f.write_str(m),
        }
    }
}

struct Opt {
    names: &'static [&'static str],
    /// Placeholder of the value, `None` for a flag.
    value: Option<&'static str>,
    help: &'static str,
}

const fn flag(names: &'static [&'static str], help: &'static str) -> Opt {
    Opt {
        names,
        value: None,
        help,
    }
}

const fn val(names: &'static [&'static str], value: &'static str, help: &'static str) -> Opt {
    Opt {
        names,
        value: Some(value),
        help,
    }
}

const OPTIONS: &[Opt] = &[
    val(&["-d", "--output-dir"], "<dir>", "output directory"),
    val(
        &["-ds", "--output-dir-src"],
        "<dir>",
        "output directory for sources",
    ),
    val(
        &["-dr", "--output-dir-res"],
        "<dir>",
        "output directory for resources",
    ),
    flag(&["-r", "--no-res"], "do not decode resources"),
    flag(&["-s", "--no-src"], "do not decompile source code"),
    val(
        &["-j", "--threads-count"],
        "<n>",
        "processing threads count, default: half of CPU threads",
    ),
    val(
        &["--single-class"],
        "<name>",
        "decompile a single class, full name or raw name",
    ),
    val(
        &["--single-class-output"],
        "<path>",
        "file or dir for write if decompile a single class",
    ),
    val(
        &["--output-format"],
        "<format>",
        "can be 'java' or 'json', default: java",
    ),
    flag(
        &["-e", "--export-gradle"],
        "save as gradle project (set '--export-gradle-type' to 'auto')",
    ),
    val(
        &["--export-gradle-type"],
        "<type>",
        "Gradle project template for export:\n 'auto' - detect automatically\n 'android-app' - Android Application (apk)\n 'android-library' - Android Library (aar)\n 'simple-java' - simple Java",
    ),
    val(
        &["-m", "--decompilation-mode"],
        "<mode>",
        "code output mode:\n 'auto' - trying best options (default)\n 'restructure' - restore code structure (normal java code)\n 'simple' - simplified instructions (linear, with goto's)\n 'fallback' - raw instructions without modifications",
    ),
    flag(
        &["--show-bad-code"],
        "show inconsistent code (incorrectly decompiled)",
    ),
    flag(&["--no-xml-pretty-print"], "do not prettify XML"),
    flag(
        &["--no-imports"],
        "disable use of imports, always write entire package name",
    ),
    flag(
        &["--no-debug-info"],
        "disable debug info parsing and processing",
    ),
    flag(
        &["--add-debug-lines"],
        "add comments with debug line numbers if available",
    ),
    flag(
        &["--no-inline-anonymous"],
        "disable anonymous classes inline",
    ),
    flag(&["--no-inline-methods"], "disable methods inline"),
    flag(
        &["--no-move-inner-classes"],
        "disable move inner classes into parent",
    ),
    flag(
        &["--no-inline-kotlin-lambda"],
        "disable inline for Kotlin lambdas",
    ),
    flag(&["--no-finally"], "don't extract finally block"),
    flag(
        &["--no-restore-switch-over-string"],
        "don't restore switch over string",
    ),
    flag(
        &["--no-replace-consts"],
        "don't replace constant value with matching constant field",
    ),
    flag(
        &["--escape-unicode"],
        "escape non latin characters in strings (with \\u)",
    ),
    flag(
        &["--respect-bytecode-access-modifiers"],
        "don't change original access modifiers",
    ),
    val(
        &["--mappings-path"],
        "<path>",
        "deobfuscation mappings file or directory. Allowed formats: Tiny and Tiny v2 (both '.tiny'), Enigma (.mapping) or Enigma directory",
    ),
    val(
        &["--mappings-mode"],
        "<mode>",
        "set mode for handling the deobfuscation mapping file:\n 'read' - just read, user can always save manually (default)\n 'read-and-autosave-every-change' - read and autosave after every change\n 'read-and-autosave-before-closing' - read and autosave before exiting the app or closing the project\n 'ignore' - don't read or save (can be used to skip loading mapping files referenced in the project file)",
    ),
    flag(&["--deobf"], "activate deobfuscation"),
    val(
        &["--deobf-min"],
        "<n>",
        "min length of name, renamed if shorter, default: 3",
    ),
    val(
        &["--deobf-max"],
        "<n>",
        "max length of name, renamed if longer, default: 64",
    ),
    val(
        &["--deobf-whitelist"],
        "<list>",
        "space separated list of classes (full name) and packages (ends with '.*') to exclude from deobfuscation, default: android.support.v4.* android.support.v7.* android.support.v4.os.* android.support.annotation.Px androidx.core.os.* androidx.annotation.Px",
    ),
    val(
        &["--deobf-cfg-file"],
        "<file>",
        "deobfuscation mappings file used for JADX auto-generated names (in the JOBF file format), default: same dir and name as input file with '.jobf' extension",
    ),
    val(
        &["--deobf-cfg-file-mode"],
        "<mode>",
        "set mode for handling the JADX auto-generated names' deobfuscation map file:\n 'read' - read if found, don't save (default)\n 'read-or-save' - read if found, save otherwise (don't overwrite)\n 'overwrite' - don't read, always save\n 'ignore' - don't read and don't save",
    ),
    val(
        &["--deobf-res-name-source"],
        "<source>",
        "better name source for resources:\n 'auto' - automatically select best name (default)\n 'resources' - use resources names\n 'code' - use R class fields names",
    ),
    val(
        &["--use-source-name-as-class-name-alias"],
        "<mode>",
        "use source name as class name alias:\n 'always' - always use source name if it's available\n 'if-better' - use source name if it seems better than the current one\n 'never' - never use source name, even if it's available",
    ),
    val(
        &["--source-name-repeat-limit"],
        "<n>",
        "allow using source name if it appears less than a limit number, default: 10",
    ),
    val(
        &["--use-kotlin-methods-for-var-names"],
        "<mode>",
        "use kotlin intrinsic methods to rename variables, values: disable, apply, apply-and-hide, default: apply",
    ),
    flag(
        &["--use-headers-for-detect-resource-extensions"],
        "Use headers for detect resource extensions if resource obfuscated",
    ),
    val(
        &["--rename-flags"],
        "<flags>",
        "fix options (comma-separated list of):\n 'case' - fix case sensitivity issues (according to --fs-case-sensitive option),\n 'valid' - rename java identifiers to make them valid,\n 'printable' - remove non-printable chars from identifiers,\nor single 'none' - to disable all renames\nor single 'all' - to enable all (default)",
    ),
    val(
        &["--integer-format"],
        "<format>",
        "how integers are displayed:\n 'auto' - automatically select (default)\n 'decimal' - use decimal\n 'hexadecimal' - use hexadecimal",
    ),
    val(
        &["--type-update-limit"],
        "<n>",
        "type update limit count (per one instruction), default: 10",
    ),
    flag(
        &["--fs-case-sensitive"],
        "treat filesystem as case sensitive, false by default",
    ),
    flag(
        &["--cfg"],
        "save methods control flow graph to dot file, not implemented yet",
    ),
    flag(
        &["--raw-cfg"],
        "save methods control flow graph (use raw instructions), not implemented yet",
    ),
    val(
        &["--call-graph"],
        "<format>",
        "save app call graph in format: 'dot' or 'json', default: none, not implemented yet",
    ),
    flag(
        &["-f", "--fallback"],
        "set '--decompilation-mode' to 'fallback' (deprecated)",
    ),
    flag(&["--use-dx"], "use dx/d8 to convert java bytecode"),
    val(
        &["--comments-level"],
        "<level>",
        "set code comments level, values: error, warn, info, debug, user-only, none, default: info",
    ),
    val(
        &["--log-level"],
        "<level>",
        "set log level, values: quiet, progress, error, warn, info, debug, default: progress",
    ),
    flag(&["-v", "--verbose"], "verbose output (set --log-level to DEBUG)"),
    flag(&["-q", "--quiet"], "turn off output (set --log-level to QUIET)"),
    val(
        &["--disable-plugins"],
        "<ids>",
        "comma separated list of plugin ids to disable",
    ),
    val(
        &["--config"],
        "<config-ref>",
        "load configuration from file, not implemented yet",
    ),
    val(
        &["--save-config"],
        "<config-ref>",
        "save current options into configuration file and exit, not implemented yet",
    ),
    flag(
        &["--print-files"],
        "print files and directories used by jadx (config, cache, temp)",
    ),
    flag(&["--version"], "print jadx version"),
    flag(&["-h", "--help"], "print this help"),
];

/// Result of the command line parsing.
#[derive(Debug)]
pub struct Cli {
    pub args: JadxArgs,
    pub log_level: LogLevel,
    pub help: bool,
    pub version: bool,
    pub single_class: Option<String>,
    pub single_class_output: Option<String>,
    pub call_graph: CallGraphMode,
    /// Accepted options that have no effect yet (printed as a warning).
    pub unimplemented: Vec<String>,
}

impl Cli {
    fn new() -> Cli {
        let args = JadxArgs {
            deobfuscation_min_length: 3,
            deobfuscation_max_length: 64,
            ..JadxArgs::default()
        };
        Cli {
            args,
            log_level: LogLevel::Progress,
            help: false,
            version: false,
            single_class: None,
            single_class_output: None,
            call_graph: CallGraphMode::None,
            unimplemented: Vec::new(),
        }
    }
}

fn find(name: &str) -> Option<&'static Opt> {
    OPTIONS.iter().find(|o| o.names.contains(&name))
}

/// Usage text with all options.
pub fn usage() -> String {
    let mut s = format!(
        "jadx-rs - dex to java decompiler, version: {}\n\nusage: jadx-rs [options] <input files> (.apk, .dex, .jar, .zip)\noptions:\n",
        jadx_api::JadxDecompiler::version()
    );
    let width = 46;
    for o in OPTIONS {
        let mut head = o.names.join(", ");
        if let Some(v) = o.value {
            head.push(' ');
            head.push_str(v);
        }
        let mut lines = o.help.split('\n');
        let first = lines.next().unwrap_or("");
        s.push_str(&format!("  {head:<width$}  - {first}\n"));
        for l in lines {
            s.push_str(&format!("  {:<width$}     {l}\n", ""));
        }
    }
    s.push_str(&format!(
        "  {:<width$}  - plugin options, accepted and stored\n",
        "-P<name>=<value>"
    ));
    s.push_str(
        "\nExamples:\n  jadx-rs -d out classes.dex\n  jadx-rs --rename-flags \"none\" classes.dex\n  jadx-rs --log-level ERROR app.apk\n",
    );
    s
}

fn enum_value<T>(
    opt: &str,
    value: &str,
    parse: impl Fn(&str) -> Option<T>,
    names: impl Fn() -> String,
) -> Result<T, ParseError> {
    parse(value).ok_or_else(|| {
        ParseError::Invalid(format!(
            "'{value}' is unknown for parameter {opt}, possible values are {}",
            names()
        ))
    })
}

fn int_value(opt: &str, value: &str) -> Result<i64, ParseError> {
    value.trim().parse::<i64>().map_err(|_| {
        ParseError::Usage(format!(
            "Invalid value for option {opt}: '{value}' is not an integer"
        ))
    })
}

fn count_value(opt: &str, value: &str) -> Result<u32, ParseError> {
    let n = int_value(opt, value)?;
    u32::try_from(n).map_err(|_| {
        ParseError::Usage(format!(
            "Invalid value for option {opt}: '{value}' is out of range"
        ))
    })
}

/// Parse the command line (without the program name).
pub fn parse(argv: &[String]) -> Result<Cli, ParseError> {
    let mut cli = Cli::new();
    let mut files: Vec<String> = Vec::new();
    let mut quiet = false;
    let mut verbose = false;
    let mut fallback = false;
    let mut export_gradle = false;
    let mut i = 0;
    let mut only_files = false;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if only_files || !arg.starts_with('-') || arg == "-" {
            files.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_files = true;
            continue;
        }
        if let Some(rest) = arg.strip_prefix("-P") {
            let (k, v) = rest.split_once('=').unwrap_or((rest, ""));
            if k.is_empty() {
                return Err(ParseError::Usage("Invalid plugin option: -P".into()));
            }
            cli.args.plugin_options.insert(k.to_string(), v.to_string());
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let Some(opt) = find(name) else {
            // like the reference, an unknown option ends up as an input file and is rejected
            return Err(ParseError::Invalid(format!("Unknown option: {arg}")));
        };
        let value = if opt.value.is_some() {
            match inline {
                Some(v) => v,
                None => {
                    if i >= argv.len() {
                        return Err(ParseError::Usage(format!(
                            "Expected a value after parameter {name}"
                        )));
                    }
                    i += 1;
                    argv[i - 1].clone()
                }
            }
        } else {
            if inline.is_some() {
                return Err(ParseError::Usage(format!(
                    "Option {name} does not take a value"
                )));
            }
            String::new()
        };
        let a = &mut cli.args;
        match opt.names[opt.names.len() - 1] {
            "--output-dir" => a.out_dir = Some(PathBuf::from(&value)),
            "--output-dir-src" => a.out_dir_src = Some(PathBuf::from(&value)),
            "--output-dir-res" => a.out_dir_res = Some(PathBuf::from(&value)),
            "--no-res" => a.skip_resources = true,
            "--no-src" => a.skip_sources = true,
            "--threads-count" => {
                let n = int_value(name, &value)?;
                if n <= 0 {
                    return Err(ParseError::Invalid(format!(
                        "Threads count must be positive, got: {n}"
                    )));
                }
                a.threads_count = usize::try_from(n).unwrap_or(usize::MAX);
            }
            "--single-class" => cli.single_class = Some(value),
            "--single-class-output" => cli.single_class_output = Some(value),
            "--output-format" => {
                a.output_format =
                    enum_value(name, &value, OutputFormat::from_name, OutputFormat::names)?
            }
            "--export-gradle" => export_gradle = true,
            "--export-gradle-type" => {
                a.export_gradle_type = Some(enum_value(
                    name,
                    &value,
                    ExportGradleType::from_name,
                    ExportGradleType::names,
                )?)
            }
            "--decompilation-mode" => {
                a.decompilation_mode = enum_value(
                    name,
                    &value,
                    DecompilationMode::from_name,
                    DecompilationMode::names,
                )?
            }
            "--show-bad-code" => a.show_inconsistent_code = true,
            "--no-xml-pretty-print" => a.skip_xml_pretty_print = true,
            "--no-imports" => a.use_imports = false,
            "--no-debug-info" => a.debug_info = false,
            "--add-debug-lines" => a.insert_debug_lines = true,
            "--no-inline-anonymous" => a.inline_anonymous_classes = false,
            "--no-inline-methods" => a.inline_methods = false,
            "--no-move-inner-classes" => a.move_inner_classes = false,
            "--no-inline-kotlin-lambda" => a.allow_inline_kotlin_lambda = false,
            "--no-finally" => a.extract_finally = false,
            "--no-restore-switch-over-string" => a.restore_switch_over_string = false,
            "--no-replace-consts" => a.replace_consts = false,
            "--escape-unicode" => a.escape_unicode = true,
            "--respect-bytecode-access-modifiers" => a.respect_bytecode_acc_modifiers = true,
            "--mappings-path" => a.user_renames_mappings_path = Some(PathBuf::from(&value)),
            "--mappings-mode" => {
                a.user_renames_mappings_mode = enum_value(
                    name,
                    &value,
                    UserRenamesMappingsMode::from_name,
                    UserRenamesMappingsMode::names,
                )?
            }
            "--deobf" => a.deobfuscation_on = true,
            "--deobf-min" => a.deobfuscation_min_length = count_value(name, &value)? as usize,
            "--deobf-max" => a.deobfuscation_max_length = count_value(name, &value)? as usize,
            "--deobf-whitelist" => {
                a.deobfuscation_whitelist = value.split(' ').map(str::to_string).collect()
            }
            "--deobf-cfg-file" => a.generated_renames_mapping_file = Some(PathBuf::from(&value)),
            "--deobf-cfg-file-mode" => {
                a.generated_renames_mapping_file_mode = enum_value(
                    name,
                    &value,
                    GeneratedRenamesMappingFileMode::from_name,
                    GeneratedRenamesMappingFileMode::names,
                )?
            }
            "--deobf-res-name-source" => {
                a.resource_name_source = enum_value(
                    name,
                    &value,
                    ResourceNameSource::from_name,
                    ResourceNameSource::names,
                )?
            }
            "--use-source-name-as-class-name-alias" => {
                a.use_source_name_as_class_name_alias = enum_value(
                    name,
                    &value,
                    UseSourceNameAsClassNameAlias::from_name,
                    UseSourceNameAsClassNameAlias::names,
                )?
            }
            "--source-name-repeat-limit" => a.source_name_repeat_limit = count_value(name, &value)?,
            "--use-kotlin-methods-for-var-names" => {
                a.use_kotlin_methods_for_var_names = enum_value(
                    name,
                    &value,
                    UseKotlinMethodsForVarNames::from_name,
                    UseKotlinMethodsForVarNames::names,
                )?
            }
            "--use-headers-for-detect-resource-extensions" => {
                a.use_headers_for_detect_resource_extensions = true
            }
            "--rename-flags" => {
                a.rename_flags = parse_rename_flags(name, &value)?;
            }
            "--integer-format" => {
                a.integer_format =
                    enum_value(name, &value, IntegerFormat::from_name, IntegerFormat::names)?
            }
            "--type-update-limit" => a.type_updates_limit_count = count_value(name, &value)?,
            "--fs-case-sensitive" => a.fs_case_sensitive = true,
            "--cfg" => {
                a.cfg_output = true;
                cli.unimplemented.push("--cfg".into());
            }
            "--raw-cfg" => {
                a.raw_cfg_output = true;
                cli.unimplemented.push("--raw-cfg".into());
            }
            "--call-graph" => {
                cli.call_graph = match value.trim().to_ascii_lowercase().as_str() {
                    "none" => CallGraphMode::None,
                    "dot" => CallGraphMode::Dot,
                    "json" => CallGraphMode::Json,
                    _ => {
                        return Err(ParseError::Invalid(format!(
                            "'{value}' is unknown for parameter {name}, possible values are none, dot, json"
                        )))
                    }
                };
                if cli.call_graph != CallGraphMode::None {
                    cli.unimplemented.push("--call-graph".into());
                }
            }
            "--fallback" => fallback = true,
            "--use-dx" => a.use_dx_input = true,
            "--comments-level" => {
                a.comments_level =
                    enum_value(name, &value, CommentsLevel::from_name, CommentsLevel::names)?
            }
            "--log-level" => {
                cli.log_level = enum_value(name, &value, LogLevel::from_name, LogLevel::names)?
            }
            "--verbose" => verbose = true,
            "--quiet" => quiet = true,
            "--disable-plugins" => {
                a.disabled_plugins = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }
            "--config" => cli.unimplemented.push("--config".into()),
            "--save-config" => cli.unimplemented.push("--save-config".into()),
            "--print-files" => cli.unimplemented.push("--print-files".into()),
            "--version" => cli.version = true,
            "--help" => cli.help = true,
            other => unreachable!("option without handler: {other}"),
        }
    }
    if quiet {
        cli.log_level = LogLevel::Quiet;
    } else if verbose {
        cli.log_level = LogLevel::Debug;
    }
    if fallback {
        cli.args.decompilation_mode = DecompilationMode::Fallback;
    }
    if export_gradle && cli.args.export_gradle_type.is_none() {
        cli.args.export_gradle_type = Some(ExportGradleType::Auto);
    }
    if let Some(p) = &cli.args.user_renames_mappings_path {
        let _ = p;
        cli.unimplemented.push("--mappings-path".into());
    }
    cli.args.input_files = files.into_iter().map(PathBuf::from).collect();
    Ok(cli)
}

fn parse_rename_flags(
    opt: &str,
    value: &str,
) -> Result<std::collections::BTreeSet<RenameFlag>, ParseError> {
    let v = value.trim();
    if v.eq_ignore_ascii_case("none") {
        return Ok(Default::default());
    }
    if v.eq_ignore_ascii_case("all") {
        return Ok(RenameFlag::ALL.iter().copied().collect());
    }
    let mut set = std::collections::BTreeSet::new();
    for part in v.split(',') {
        let flag = RenameFlag::from_name(part).ok_or_else(|| {
            ParseError::Invalid(format!(
                "'{part}' is unknown for parameter {opt}, possible values are {}",
                RenameFlag::names()
            ))
        })?;
        set.insert(flag);
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Cli, ParseError> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn basic_options() {
        let c = p(&[
            "-d",
            "out",
            "-j",
            "3",
            "-r",
            "--show-bad-code",
            "a.apk",
            "b.dex",
        ])
        .unwrap();
        assert_eq!(c.args.out_dir, Some(PathBuf::from("out")));
        assert_eq!(c.args.threads_count, 3);
        assert!(c.args.skip_resources && !c.args.skip_sources);
        assert!(c.args.show_inconsistent_code);
        assert_eq!(c.args.input_files.len(), 2);
    }

    #[test]
    fn inverted_flags_and_equals_form() {
        let c = p(&["--no-imports", "--no-finally", "--log-level=debug", "x.dex"]).unwrap();
        assert!(!c.args.use_imports && !c.args.extract_finally);
        assert_eq!(c.log_level, LogLevel::Debug);
    }

    #[test]
    fn quiet_and_verbose() {
        assert_eq!(p(&["-q", "x"]).unwrap().log_level, LogLevel::Quiet);
        assert_eq!(p(&["-v", "x"]).unwrap().log_level, LogLevel::Debug);
        assert_eq!(p(&["-v", "-q", "x"]).unwrap().log_level, LogLevel::Quiet);
    }

    #[test]
    fn errors() {
        assert!(matches!(p(&["--bogus"]), Err(ParseError::Invalid(_))));
        assert!(matches!(p(&["-d"]), Err(ParseError::Usage(_))));
        assert!(matches!(p(&["-j", "0", "x"]), Err(ParseError::Invalid(_))));
        assert!(matches!(p(&["-j", "x"]), Err(ParseError::Usage(_))));
        assert!(matches!(
            p(&["--log-level", "loud"]),
            Err(ParseError::Invalid(_))
        ));
    }

    #[test]
    fn deobf_and_rename_flags() {
        let c = p(&[
            "--deobf",
            "--deobf-min",
            "2",
            "--deobf-max",
            "40",
            "--rename-flags",
            "valid,printable",
            "x",
        ])
        .unwrap();
        assert!(c.args.deobfuscation_on);
        assert_eq!(c.args.deobfuscation_min_length, 2);
        assert_eq!(c.args.deobfuscation_max_length, 40);
        assert_eq!(c.args.rename_flags.len(), 2);
        let c = p(&["--rename-flags", "none", "x"]).unwrap();
        assert!(c.args.rename_flags.is_empty());
    }

    #[test]
    fn plugin_options_and_gradle() {
        let c = p(&["-Pdex-input.verify-checksum=no", "-e", "x"]).unwrap();
        assert_eq!(
            c.args.plugin_options.get("dex-input.verify-checksum"),
            Some(&"no".to_string())
        );
        assert_eq!(c.args.export_gradle_type, Some(ExportGradleType::Auto));
    }

    #[test]
    fn help_lists_every_option() {
        let u = usage();
        for o in OPTIONS {
            assert!(u.contains(o.names[o.names.len() - 1]), "{:?}", o.names);
        }
    }
}
