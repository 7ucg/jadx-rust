//! `CodeGen`: entry point of the class code generation.

use super::class_gen::{ClassGen, GenEnv};
use super::code_writer::CodeInfo;
use super::CodegenError;

/// `CodeGen.generate`: source code of a top level class with its inner classes.
pub fn generate(env: &GenEnv<'_>, cls: usize) -> Result<CodeInfo, CodegenError> {
    if env.root.classes[cls].dont_generate {
        return Ok(CodeInfo::default());
    }
    let gen = ClassGen::new(env, cls);
    gen.make_class()
}
