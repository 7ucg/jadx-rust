//! Class level passes that run once over the whole input before any method is processed
//! (`Jadx.getPreDecompilePassesList`).

use jadx_input_api::CodeRefProvider;

use crate::model::node::Root;
use crate::passes::{
    add_android_constants, collect_const_values, override_methods, prepare_access_flags,
    process_anonymous, process_methods_for_inline, rename, signature, usage_info,
};

/// Options of the pre decompile passes (defaults of the reference decompiler).
#[derive(Debug, Clone)]
pub struct PreDecompileOptions {
    /// Keep the access flags of the bytecode, do not restore them from `InnerClass` data.
    pub respect_bytecode_acc_modifiers: bool,
    /// Collect constant fields for literal replacement.
    pub replace_consts: bool,
    /// Mark anonymous classes for inlining.
    pub inline_anonymous_classes: bool,
    /// Mark synthetic methods for inlining.
    pub inline_methods: bool,
    /// Treat classes extending the Kotlin lambda base class as inlinable.
    pub allow_inline_kotlin_lambda: bool,
    /// Rename invalid and colliding names of classes, fields, methods and packages.
    pub rename: Option<rename::RenameOptions>,
}

impl Default for PreDecompileOptions {
    fn default() -> Self {
        PreDecompileOptions {
            respect_bytecode_acc_modifiers: false,
            replace_consts: true,
            inline_anonymous_classes: true,
            inline_methods: true,
            allow_inline_kotlin_lambda: true,
            rename: Some(rename::RenameOptions::default()),
        }
    }
}

/// Run the pre decompile passes in the order of the reference:
/// access flags, signatures, method overrides, android constants, renaming, usage info, constant values, anonymous classes, methods to inline.
pub fn run_pre_decompile(root: &mut Root, refs: &dyn CodeRefProvider, opts: &PreDecompileOptions) {
    if !opts.respect_bytecode_acc_modifiers {
        prepare_access_flags::prepare_access_flags(root);
    }
    signature::process_signatures(root);
    override_methods::process_overrides(root);
    root.const_storage.set_replace_enabled(opts.replace_consts);
    add_android_constants::add_android_constants(root);
    if let Some(r) = &opts.rename {
        rename::rename_all(root, r);
    }
    usage_info::collect_usage(root, refs);
    collect_const_values::collect_const_values(root);
    if opts.inline_anonymous_classes {
        process_anonymous::process_anonymous(root, opts.allow_inline_kotlin_lambda);
    }
    if opts.inline_methods {
        process_methods_for_inline::process_methods_for_inline(root, opts.inline_anonymous_classes);
    }
}
