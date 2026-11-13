//! Public decompiler API: arguments, the [`JadxDecompiler`] facade, views of classes, packages
//! and resources, pluggable class code generation and the output writer.
//!
//! ```no_run
//! use jadx_api::{JadxArgs, JadxDecompiler};
//!
//! let mut args = JadxArgs::default();
//! args.add_input_file("app.apk");
//! args.set_root_dir("out");
//! let mut jadx = JadxDecompiler::new(args);
//! jadx.load().unwrap();
//! for cls in jadx.classes() {
//!     println!("{}", cls.full_name());
//! }
//! jadx.save().unwrap();
//! ```

pub use jadx_core::access;
pub mod args;
pub mod code;
pub mod core_gen;
pub mod decompiler;
pub mod error;
pub mod generator;
pub mod input_code;
pub mod java;
pub mod json_gen;
pub mod log;
pub mod parallel;
pub mod paths;
pub mod resources;
pub mod scheduler;

pub use access::{AccessInfo, AccessKind};
pub use args::{
    default_threads_count, ClassFilter, CommentsLevel, DecompilationMode, ExportGradleType,
    GeneratedRenamesMappingFileMode, IntegerFormat, JadxArgs, OutputFormat, RenameFlag,
    ResourceNameSource, UseKotlinMethodsForVarNames, UseSourceNameAsClassNameAlias,
    UserRenamesMappingsMode,
};
pub use code::{CodeWriter, ICodeInfo, SimpleCodeInfo};
pub use core_gen::CoreClassGenerator;
pub use decompiler::{class_output_path, ClassResult, JadxDecompiler, ProgressFn};
pub use error::{ErrorsCounter, JadxError, Result};
pub use generator::{
    ClassCodeGenerator, ClassDiagnostics, DeclarationGenerator, GenContext, PLACEHOLDER_BODY,
};
pub use input_code::{ClassOrigin, CombinedCodeRefs, MethodCode};
pub use java::{JavaClass, JavaField, JavaMethod, JavaPackage};
pub use log::{Level, LogSink, NullLog};
pub use resources::{
    AndroidResourceDecoder, DecodedResFile, DecodedResources, ResourceContext, ResourceDecoder,
    ResourceFile, ResourceType,
};
