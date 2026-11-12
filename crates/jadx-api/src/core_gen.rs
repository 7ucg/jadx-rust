//! `CoreClassGenerator`: class code from the processed code of the methods (the full per-method
//! pass pipeline of the core).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

use jadx_core::codegen::method_gen::no_code_cfg;
use jadx_core::codegen::prepare::prepare_variables;
use jadx_core::codegen::string_utils::{IntegerFormat as CoreIntegerFormat, StringUtils};
use jadx_core::codegen::{
    code_gen, CommentsLevel as CoreCommentsLevel, DecompilationMode as CoreMode, GenEnv,
    MethodLoader,
};
use jadx_core::decode::{InsnDecoder, MethodCtx};
use jadx_core::model::cfg::MethodCfg;
use jadx_core::passes::pipeline::{build_ssa, FrameInfo};

use crate::args::{CommentsLevel, DecompilationMode, IntegerFormat};
use crate::code::SimpleCodeInfo;
use crate::generator::{ClassCodeGenerator, ClassDiagnostics, GenContext};

/// Last stage of the per-method pipeline run before the code generation.
const LAST_STAGE: u32 = 19;

/// Generates the code of classes from the decompiled method bodies.
#[derive(Default)]
pub struct CoreClassGenerator {
    /// Names of the root packages of the loaded input, by root address.
    root_pkgs: Mutex<Option<(usize, HashSet<String>)>>,
}

impl CoreClassGenerator {
    pub fn new() -> Self {
        Self::default()
    }

    fn root_pkgs(&self, ctx: &GenContext<'_>) -> HashSet<String> {
        let key = ctx.root as *const _ as usize;
        let mut guard = self.root_pkgs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, set)) = guard.as_ref() {
            if *k == key {
                return set.clone();
            }
        }
        let mut set = HashSet::new();
        for c in &ctx.root.classes {
            let pkg = c.info.package();
            if !pkg.is_empty() {
                set.insert(pkg.split('.').next().unwrap_or(pkg).to_string());
            }
        }
        *guard = Some((key, set.clone()));
        set
    }
}

fn comments_level(l: CommentsLevel) -> CoreCommentsLevel {
    match l {
        CommentsLevel::None => CoreCommentsLevel::None,
        CommentsLevel::UserOnly => CoreCommentsLevel::UserOnly,
        CommentsLevel::Error => CoreCommentsLevel::Error,
        CommentsLevel::Warn => CoreCommentsLevel::Warn,
        CommentsLevel::Info => CoreCommentsLevel::Info,
        CommentsLevel::Debug => CoreCommentsLevel::Debug,
    }
}

fn decompilation_mode(m: DecompilationMode) -> CoreMode {
    match m {
        DecompilationMode::Auto => CoreMode::Auto,
        DecompilationMode::Restructure => CoreMode::Restructure,
        DecompilationMode::Simple => CoreMode::Simple,
        DecompilationMode::Fallback => CoreMode::Fallback,
    }
}

impl ClassCodeGenerator for CoreClassGenerator {
    fn generate(
        &self,
        ctx: &GenContext<'_>,
        cls: usize,
        diag: &mut ClassDiagnostics,
    ) -> Result<SimpleCodeInfo, String> {
        let loader = DexMethodLoader {
            ctx,
            lines: RefCell::new(HashMap::new()),
        };
        let mut env =
            GenEnv::with_inner_classes(ctx.root, ctx.inner_classes.to_vec(), self.root_pkgs(ctx));
        env.use_imports = ctx.args.use_imports;
        env.fallback = matches!(ctx.args.decompilation_mode, DecompilationMode::Fallback);
        env.comments_level = comments_level(ctx.args.comments_level);
        env.show_inconsistent_code = ctx.args.show_inconsistent_code;
        env.decompilation_mode = decompilation_mode(ctx.args.decompilation_mode);
        env.code_new_line_str = ctx.args.code_new_line_str.clone();
        env.code_indent_str = ctx.args.code_indent_str.clone();
        env.string_utils = StringUtils {
            escape_unicode: ctx.args.escape_unicode,
            integer_format: match ctx.args.integer_format {
                IntegerFormat::Auto => CoreIntegerFormat::Auto,
                IntegerFormat::Decimal => CoreIntegerFormat::Decimal,
                IntegerFormat::Hexadecimal => CoreIntegerFormat::Hexadecimal,
            },
        };
        env.loader = Some(&loader);
        let generated = code_gen::generate(&env, cls).map_err(|e| e.msg);
        for e in env.errors() {
            diag.error(e);
        }
        let info = generated?;
        Ok(SimpleCodeInfo::new(info.code))
    }
}

/// Loads the code of methods from the dex files and runs the pass pipeline.
struct DexMethodLoader<'a> {
    ctx: &'a GenContext<'a>,
    lines: RefCell<HashMap<(usize, usize), u32>>,
}

impl DexMethodLoader<'_> {
    fn process(&self, cls: usize, mth: usize) -> Result<MethodCfg, String> {
        let root = self.ctx.root;
        let m = &root.classes[cls].methods[mth];
        let Some(off) = m.code_offset else {
            return Ok(no_code_cfg(root, cls, mth));
        };
        let dex = self.ctx.dex_of(cls);
        let code = dex.code_item(off).map_err(|e| e.to_string())?;
        let raw =
            jadx_dex_input::insns::decode_insn_data(&code.insns).map_err(|e| e.to_string())?;
        let mctx = MethodCtx {
            return_type: m.return_type.clone(),
            parent_class: root.classes[cls].info.clone(),
            root,
        };
        let decoded = InsnDecoder::new(dex, &mctx)
            .process(&raw, code.insns.len())
            .map_err(|e| e.to_string())?;
        let tries = dex.tries_api(&code).map_err(|e| e.to_string())?;
        let frame = FrameInfo::new(root, cls, mth, code.registers_size as usize);
        let mut cfg = build_ssa(
            decoded,
            &tries,
            root,
            cls,
            mth,
            &frame,
            LAST_STAGE,
            self.ctx.args.extract_finally,
        )?;
        prepare_variables(&mut cfg, root);
        Ok(cfg)
    }
}

impl MethodLoader for DexMethodLoader<'_> {
    fn load(&self, cls: usize, mth: usize) -> Result<MethodCfg, String> {
        catch_unwind(AssertUnwindSafe(|| self.process(cls, mth)))
            .unwrap_or_else(|_| Err("panic in method processing".to_string()))
    }

    fn source_line(&self, cls: usize, mth: usize) -> u32 {
        if let Some(l) = self.lines.borrow().get(&(cls, mth)) {
            return *l;
        }
        let l = self.compute_source_line(cls, mth);
        self.lines.borrow_mut().insert((cls, mth), l);
        l
    }
}

impl DexMethodLoader<'_> {
    /// `DebugInfoAttachVisitor.setMethodSourceLine`: line of the first instruction with a line
    /// number, minus one.
    fn compute_source_line(&self, cls: usize, mth: usize) -> u32 {
        let m = &self.ctx.root.classes[cls].methods[mth];
        let Some(off) = m.code_offset else {
            return 0;
        };
        let dex = self.ctx.dex_of(cls);
        let Ok(code) = dex.code_item(off) else {
            return 0;
        };
        if code.debug_info_off == 0 {
            return 0;
        }
        let Ok(lines) = dex.debug_lines(code.debug_info_off, code.insns.len() as u32) else {
            return 0;
        };
        let mut map: HashMap<u32, u32> = HashMap::new();
        for (addr, line) in lines {
            map.insert(addr, line);
        }
        let Ok(raw) = jadx_dex_input::insns::decode_insn_data(&code.insns) else {
            return 0;
        };
        for insn in &raw {
            if let Some(&line) = map.get(&insn.offset) {
                if line != 0 {
                    return line - 1;
                }
            }
        }
        0
    }
}
