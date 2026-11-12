//! Decompiler arguments (`JadxArgs` and the option enums of the reference).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

/// Defines an option enum with the command line names of its values.
macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($var:ident = $text:literal),+ $(,)? } default $def:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $($var),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];

            /// Name used on the command line.
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$var => $text),+
                }
            }

            /// Parse a command line value (case insensitive, `_` accepted for `-`).
            pub fn from_name(s: &str) -> Option<$name> {
                let norm = s.trim().to_ascii_lowercase().replace('_', "-");
                $(if norm == $text { return Some($name::$var); })+
                None
            }

            /// All command line names, comma separated.
            pub fn names() -> String {
                [$($text),+].join(", ")
            }
        }

        impl Default for $name {
            fn default() -> Self {
                $name::$def
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum! {
    /// Format of the generated class files.
    OutputFormat { Java = "java", Json = "json" } default Java
}

impl OutputFormat {
    /// File extension of saved classes, with the leading dot.
    pub fn file_extension(self) -> &'static str {
        match self {
            OutputFormat::Java => ".java",
            OutputFormat::Json => ".json",
        }
    }
}

string_enum! {
    DecompilationMode {
        Auto = "auto",
        Restructure = "restructure",
        Simple = "simple",
        Fallback = "fallback",
    } default Auto
}

string_enum! {
    CommentsLevel {
        None = "none",
        UserOnly = "user-only",
        Error = "error",
        Warn = "warn",
        Info = "info",
        Debug = "debug",
    } default Info
}

impl CommentsLevel {
    /// True if a comment of this level is shown with `limit` as the configured level.
    pub fn filter(self, limit: CommentsLevel) -> bool {
        self <= limit
    }
}

string_enum! {
    IntegerFormat { Auto = "auto", Decimal = "decimal", Hexadecimal = "hexadecimal" } default Auto
}

string_enum! {
    ResourceNameSource { Auto = "auto", Resources = "resources", Code = "code" } default Auto
}

string_enum! {
    UseSourceNameAsClassNameAlias {
        Always = "always",
        IfBetter = "if-better",
        Never = "never",
    } default Never
}

string_enum! {
    UserRenamesMappingsMode {
        Read = "read",
        ReadAndAutosaveEveryChange = "read-and-autosave-every-change",
        ReadAndAutosaveBeforeClosing = "read-and-autosave-before-closing",
        Ignore = "ignore",
    } default Read
}

string_enum! {
    GeneratedRenamesMappingFileMode {
        Read = "read",
        ReadOrSave = "read-or-save",
        Overwrite = "overwrite",
        Ignore = "ignore",
    } default Read
}

string_enum! {
    UseKotlinMethodsForVarNames {
        Disable = "disable",
        Apply = "apply",
        ApplyAndHide = "apply-and-hide",
    } default Apply
}

string_enum! {
    RenameFlag { Case = "case", Valid = "valid", Printable = "printable" } default Valid
}

string_enum! {
    ExportGradleType {
        Auto = "auto",
        AndroidApp = "android-app",
        AndroidLibrary = "android-library",
        SimpleJava = "simple-java",
    } default Auto
}

/// Classes and packages (ends with `.*`) that are not renamed by the deobfuscation.
pub const DEFAULT_DEOBF_WHITELIST: &[&str] = &[
    "android.support.v4.*",
    "android.support.v7.*",
    "android.support.v4.os.*",
    "android.support.annotation.Px",
    "androidx.core.os.*",
    "androidx.annotation.Px",
];

pub const DEFAULT_OUT_DIR: &str = "jadx-output";
pub const DEFAULT_SRC_DIR: &str = "sources";
pub const DEFAULT_RES_DIR: &str = "resources";

/// Default number of processing threads: half of the available CPU threads, at least one.
pub fn default_threads_count() -> usize {
    (std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        / 2)
    .max(1)
}

/// Predicate over the full name of a class: only accepted classes are saved.
#[derive(Clone)]
pub struct ClassFilter(pub Arc<dyn Fn(&str) -> bool + Send + Sync>);

impl fmt::Debug for ClassFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClassFilter(..)")
    }
}

/// All settings of a decompiler run. Field names follow the reference `JadxArgs`.
#[derive(Debug, Clone)]
pub struct JadxArgs {
    pub input_files: Vec<PathBuf>,
    pub out_dir: Option<PathBuf>,
    pub out_dir_src: Option<PathBuf>,
    pub out_dir_res: Option<PathBuf>,
    pub output_format: OutputFormat,
    pub threads_count: usize,
    pub cfg_output: bool,
    pub raw_cfg_output: bool,
    pub show_inconsistent_code: bool,
    pub use_imports: bool,
    pub debug_info: bool,
    pub insert_debug_lines: bool,
    pub extract_finally: bool,
    pub inline_anonymous_classes: bool,
    pub inline_methods: bool,
    pub allow_inline_kotlin_lambda: bool,
    pub move_inner_classes: bool,
    pub restore_switch_over_string: bool,
    pub skip_resources: bool,
    pub skip_sources: bool,
    /// Do not write any files (classes are still processed).
    pub skip_files_save: bool,
    pub use_headers_for_detect_resource_extensions: bool,
    pub class_filter: Option<ClassFilter>,
    /// Save the dependencies of the classes accepted by `class_filter` too.
    pub include_dependencies: bool,
    pub user_renames_mappings_path: Option<PathBuf>,
    pub user_renames_mappings_mode: UserRenamesMappingsMode,
    pub deobfuscation_on: bool,
    pub use_source_name_as_class_name_alias: UseSourceNameAsClassNameAlias,
    pub source_name_repeat_limit: u32,
    pub generated_renames_mapping_file: Option<PathBuf>,
    pub generated_renames_mapping_file_mode: GeneratedRenamesMappingFileMode,
    pub resource_name_source: ResourceNameSource,
    pub deobfuscation_min_length: usize,
    pub deobfuscation_max_length: usize,
    pub deobfuscation_whitelist: Vec<String>,
    pub escape_unicode: bool,
    pub replace_consts: bool,
    pub respect_bytecode_acc_modifiers: bool,
    pub export_gradle_type: Option<ExportGradleType>,
    pub skip_xml_pretty_print: bool,
    pub decompilation_mode: DecompilationMode,
    pub rename_flags: BTreeSet<RenameFlag>,
    pub fs_case_sensitive: bool,
    pub comments_level: CommentsLevel,
    pub integer_format: IntegerFormat,
    pub type_updates_limit_count: u32,
    pub use_dx_input: bool,
    pub use_kotlin_methods_for_var_names: UseKotlinMethodsForVarNames,
    /// Verify the checksum of dex files before loading.
    pub verify_dex_checksum: bool,
    /// `-P<name>=<value>` plugin options.
    pub plugin_options: BTreeMap<String, String>,
    pub disabled_plugins: BTreeSet<String>,
    pub code_new_line_str: String,
    pub code_indent_str: String,
}

impl Default for JadxArgs {
    fn default() -> Self {
        JadxArgs {
            input_files: Vec::new(),
            out_dir: None,
            out_dir_src: None,
            out_dir_res: None,
            output_format: OutputFormat::Java,
            threads_count: default_threads_count(),
            cfg_output: false,
            raw_cfg_output: false,
            show_inconsistent_code: false,
            use_imports: true,
            debug_info: true,
            insert_debug_lines: false,
            extract_finally: true,
            inline_anonymous_classes: true,
            inline_methods: true,
            allow_inline_kotlin_lambda: true,
            move_inner_classes: true,
            restore_switch_over_string: true,
            skip_resources: false,
            skip_sources: false,
            skip_files_save: false,
            use_headers_for_detect_resource_extensions: false,
            class_filter: None,
            include_dependencies: false,
            user_renames_mappings_path: None,
            user_renames_mappings_mode: UserRenamesMappingsMode::default(),
            deobfuscation_on: false,
            use_source_name_as_class_name_alias: UseSourceNameAsClassNameAlias::default(),
            source_name_repeat_limit: 10,
            generated_renames_mapping_file: None,
            generated_renames_mapping_file_mode: GeneratedRenamesMappingFileMode::default(),
            resource_name_source: ResourceNameSource::Auto,
            deobfuscation_min_length: 0,
            deobfuscation_max_length: usize::MAX,
            deobfuscation_whitelist: DEFAULT_DEOBF_WHITELIST
                .iter()
                .map(|s| s.to_string())
                .collect(),
            escape_unicode: false,
            replace_consts: true,
            respect_bytecode_acc_modifiers: false,
            export_gradle_type: None,
            skip_xml_pretty_print: false,
            decompilation_mode: DecompilationMode::Auto,
            rename_flags: RenameFlag::ALL.iter().copied().collect(),
            fs_case_sensitive: false,
            comments_level: CommentsLevel::Info,
            integer_format: IntegerFormat::Auto,
            type_updates_limit_count: 10,
            use_dx_input: false,
            use_kotlin_methods_for_var_names: UseKotlinMethodsForVarNames::Apply,
            verify_dex_checksum: true,
            plugin_options: BTreeMap::new(),
            disabled_plugins: BTreeSet::new(),
            code_new_line_str: "\n".to_string(),
            code_indent_str: "    ".to_string(),
        }
    }
}

impl JadxArgs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_input_file(&mut self, file: impl Into<PathBuf>) {
        self.input_files.push(file.into());
    }

    /// Set the output root: sources and resources go to the default sub directories.
    pub fn set_root_dir(&mut self, root: impl Into<PathBuf>) {
        let root = root.into();
        self.out_dir_src = Some(root.join(DEFAULT_SRC_DIR));
        self.out_dir_res = Some(root.join(DEFAULT_RES_DIR));
        self.out_dir = Some(root);
    }

    pub fn is_rename_valid(&self) -> bool {
        self.rename_flags.contains(&RenameFlag::Valid)
    }

    pub fn is_rename_printable(&self) -> bool {
        self.rename_flags.contains(&RenameFlag::Printable)
    }

    pub fn is_rename_case(&self) -> bool {
        self.rename_flags.contains(&RenameFlag::Case)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_names() {
        assert_eq!(
            UseSourceNameAsClassNameAlias::from_name("If_Better"),
            Some(UseSourceNameAsClassNameAlias::IfBetter)
        );
        assert_eq!(
            CommentsLevel::from_name("user-only"),
            Some(CommentsLevel::UserOnly)
        );
        assert_eq!(CommentsLevel::from_name("bogus"), None);
        assert_eq!(OutputFormat::default().file_extension(), ".java");
        assert!(CommentsLevel::Warn.filter(CommentsLevel::Info));
        assert!(!CommentsLevel::Debug.filter(CommentsLevel::Info));
    }

    #[test]
    fn defaults() {
        let a = JadxArgs::default();
        assert!(a.threads_count >= 1);
        assert!(a.use_imports && a.replace_consts);
        assert_eq!(a.rename_flags.len(), 3);
        assert_eq!(a.deobfuscation_whitelist.len(), 6);
    }
}
