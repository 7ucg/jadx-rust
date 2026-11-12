//! `CoreClassGenerator`: class code from the processed code of the methods (the class pass
//! driver of the core and the code generator).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

use jadx_core::codegen::method_gen::no_code_cfg;
use jadx_core::codegen::string_utils::{IntegerFormat as CoreIntegerFormat, StringUtils};
use jadx_core::codegen::var_names::apply_variable_names;
use jadx_core::codegen::{
    code_gen, CommentsLevel as CoreCommentsLevel, DecompilationMode as CoreMode, FallbackCode,
    GenEnv, MethodLoader,
};
use jadx_core::decode::{InsnDecoder, MethodCtx};
use jadx_core::model::cfg::MethodCfg;
use jadx_core::model::node::{MethodId, Root};
use jadx_core::passes::debug_info::apply_method_parameters;
use jadx_core::passes::driver::{ClassDriver, CodeSource, DriverOptions, LoadedCode, MethodState};
use jadx_core::passes::pipeline::{build_ssa, fallback_decode, FrameInfo};
use jadx_dex_input::DexFile;

use crate::args::{CommentsLevel, DecompilationMode, IntegerFormat};
use crate::code::SimpleCodeInfo;
use crate::generator::{ClassCodeGenerator, ClassDiagnostics, GenContext};
use crate::input_code::ClassOrigin;

/// Last stage of the per-method pipeline run before the code generation in fallback mode.
const FALLBACK_LAST_STAGE: u32 = 19;

/// Method code read from the loaded inputs (dex files and JVM classes).
struct OwnedCodeSource {
    dexes: Vec<DexFile>,
    java_classes: Vec<jadx_java_input::JavaClass>,
    class_origin: Vec<ClassOrigin>,
}

impl OwnedCodeSource {
    fn decode(&self, root: &Root, cls: usize, mth: usize) -> Result<LoadedCode, String> {
        let m = &root.classes[cls].methods[mth];
        let off = m.code_offset.ok_or("method without code")?;
        let mctx = MethodCtx {
            return_type: m.return_type.clone(),
            parent_class: root.classes[cls].info.clone(),
            root,
        };
        match self.class_origin.get(cls).copied() {
            Some(ClassOrigin::Dex(d)) => {
                let dex = &self.dexes[d];
                let code = dex.code_item(off).map_err(|e| e.to_string())?;
                let raw = jadx_dex_input::insns::decode_insn_data(&code.insns)
                    .map_err(|e| e.to_string())?;
                let decoded = InsnDecoder::new(dex, &mctx)
                    .process(&raw, code.insns.len())
                    .map_err(|e| e.to_string())?;
                let tries = dex.tries_api(&code).map_err(|e| e.to_string())?;
                Ok(LoadedCode {
                    decoded,
                    tries,
                    regs_count: code.registers_size as usize,
                    args_start_reg: None,
                })
            }
            Some(ClassOrigin::Java(j)) => {
                let class = &self.java_classes[j];
                let info = class.code_info(off).map_err(|e| e.to_string())?;
                let insns = class.decode_core_insns(off).map_err(|e| e.to_string())?;
                let tries = class.tries(off).map_err(|e| e.to_string())?;
                let decoded = InsnDecoder::new(class, &mctx)
                    .process(&insns, info.code_length as usize)
                    .map_err(|e| e.to_string())?;
                Ok(LoadedCode {
                    decoded,
                    tries,
                    regs_count: info.registers_count() as usize,
                    args_start_reg: Some(info.args_start_reg() as usize),
                })
            }
            _ => Err("class without code".to_string()),
        }
    }
}

impl CodeSource for OwnedCodeSource {
    fn load_code(&self, root: &Root, cls: usize, mth: usize) -> Result<LoadedCode, String> {
        self.decode(root, cls, mth)
    }
}

/// The class pass driver over a private copy of the class tree (the passes modify classes,
/// fields and methods) and the classes whose code was already generated.
struct DriverState {
    /// Address of the root the driver was created for.
    root_key: usize,
    driver: ClassDriver<'static>,
    generated: HashSet<usize>,
}

/// Generates the code of classes from the decompiled method bodies. The class passes run in a
/// [`ClassDriver`] (dependencies first), so classes are generated one at a time.
#[derive(Default)]
pub struct CoreClassGenerator {
    /// Names of the root packages of the loaded input, by root address.
    root_pkgs: Mutex<Option<(usize, HashSet<String>)>>,
    state: Mutex<Option<DriverState>>,
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

    fn new_driver(ctx: &GenContext<'_>) -> ClassDriver<'static> {
        let src = OwnedCodeSource {
            dexes: ctx.dexes.to_vec(),
            java_classes: ctx.java_classes.to_vec(),
            class_origin: ctx.class_origin.to_vec(),
        };
        let opts = DriverOptions {
            extract_finally: ctx.args.extract_finally,
            inline_methods: ctx.args.inline_methods,
            respect_bytecode_acc_modifiers: ctx.args.respect_bytecode_acc_modifiers,
            ..DriverOptions::default()
        };
        ClassDriver::new(ctx.root.clone(), src, opts)
    }

    fn make_env<'a>(
        &self,
        ctx: &GenContext<'_>,
        root: &'a Root,
        loader: &'a dyn MethodLoader,
    ) -> GenEnv<'a> {
        let mut env =
            GenEnv::with_inner_classes(root, ctx.inner_classes.to_vec(), self.root_pkgs(ctx));
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
        env.loader = Some(loader);
        env
    }

    /// Fallback mode: no class passes run, the methods only provide their arguments and the dump
    /// of the instructions.
    fn generate_fallback(
        &self,
        ctx: &GenContext<'_>,
        cls: usize,
        diag: &mut ClassDiagnostics,
    ) -> Result<SimpleCodeInfo, String> {
        let loader = FallbackLoader {
            ctx,
            lines: RefCell::new(HashMap::new()),
        };
        let env = self.make_env(ctx, ctx.root, &loader);
        let generated = code_gen::generate(&env, cls).map_err(|e| e.msg);
        for e in env.errors() {
            diag.error(e);
        }
        Ok(SimpleCodeInfo::new(generated?.code))
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
        if matches!(ctx.args.decompilation_mode, DecompilationMode::Fallback) {
            return self.generate_fallback(ctx, cls, diag);
        }
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let key = ctx.root as *const Root as usize;
        // a class generated before is processed again by a fresh driver
        let stale = guard
            .as_ref()
            .is_none_or(|s| s.root_key != key || s.generated.contains(&cls));
        if stale {
            *guard = Some(DriverState {
                root_key: key,
                driver: Self::new_driver(ctx),
                generated: HashSet::new(),
            });
        }
        let st = guard.as_mut().expect("driver state");
        let processed = catch_unwind(AssertUnwindSafe(|| st.driver.process_with_deps(cls)));
        if processed.is_err() {
            return Err("panic in class processing".to_string());
        }
        st.generated.insert(cls);
        let methods = std::mem::take(&mut st.driver.methods);
        let loader = DriverLoader {
            ctx,
            root: &st.driver.root,
            driver: &st.driver,
            methods: RefCell::new(methods),
            lines: RefCell::new(HashMap::new()),
        };
        let env = self.make_env(ctx, &st.driver.root, &loader);
        let generated = catch_unwind(AssertUnwindSafe(|| {
            code_gen::generate(&env, cls).map_err(|e| e.msg)
        }));
        for e in env.errors() {
            diag.error(e);
        }
        drop(env);
        let mut methods = loader.methods.into_inner();
        let root = &st.driver.root;
        methods.retain(|(c, _), _| root.top_parent_index(*c) != cls);
        st.driver.methods = methods;
        match generated {
            Ok(info) => Ok(SimpleCodeInfo::new(info?.code)),
            Err(_) => Err("panic in code generation".to_string()),
        }
    }
}

/// The processed code of the methods, taken from the class driver.
struct DriverLoader<'a> {
    ctx: &'a GenContext<'a>,
    root: &'a Root,
    driver: &'a ClassDriver<'static>,
    methods: RefCell<HashMap<MethodId, MethodState>>,
    lines: RefCell<HashMap<(usize, usize), u32>>,
}

impl MethodLoader for DriverLoader<'_> {
    fn load(&self, cls: usize, mth: usize) -> Result<MethodCfg, String> {
        if self.root.classes[cls].methods[mth].code_offset.is_none() {
            return Ok(no_code_cfg(self.root, cls, mth));
        }
        let Some(state) = self.methods.borrow_mut().remove(&(cls, mth)) else {
            return Err("method code is not available".to_string());
        };
        if let Some(e) = state.failed {
            return Err(e);
        }
        state
            .cfg
            .ok_or_else(|| "method code is not available".to_string())
    }

    fn fallback_code(&self, cls: usize, mth: usize) -> Result<FallbackCode, String> {
        fallback_code(self.ctx, cls, mth)
    }

    fn class_process_complete(&self, cls: usize) -> bool {
        self.driver.is_process_complete(cls)
    }

    fn source_line(&self, cls: usize, mth: usize) -> u32 {
        cached_source_line(self.ctx, &self.lines, cls, mth)
    }
}

/// Loads the code of methods for the fallback mode.
struct FallbackLoader<'a> {
    ctx: &'a GenContext<'a>,
    lines: RefCell<HashMap<(usize, usize), u32>>,
}

impl MethodLoader for FallbackLoader<'_> {
    fn load(&self, cls: usize, mth: usize) -> Result<MethodCfg, String> {
        catch_unwind(AssertUnwindSafe(|| self.process(cls, mth)))
            .unwrap_or_else(|_| Err("panic in method processing".to_string()))
    }

    fn fallback_code(&self, cls: usize, mth: usize) -> Result<FallbackCode, String> {
        fallback_code(self.ctx, cls, mth)
    }

    fn source_line(&self, cls: usize, mth: usize) -> u32 {
        cached_source_line(self.ctx, &self.lines, cls, mth)
    }
}

impl FallbackLoader<'_> {
    fn process(&self, cls: usize, mth: usize) -> Result<MethodCfg, String> {
        let root = self.ctx.root;
        let m = &root.classes[cls].methods[mth];
        let Some(off) = m.code_offset else {
            return Ok(no_code_cfg(root, cls, mth));
        };
        let code = self.ctx.method_code(cls, off)?;
        let mctx = MethodCtx {
            return_type: m.return_type.clone(),
            parent_class: root.classes[cls].info.clone(),
            root,
        };
        let decoded = InsnDecoder::new(code.resolver, &mctx)
            .process(&code.insns, code.units)
            .map_err(|e| e.to_string())?;
        let mut frame = FrameInfo::new(root, cls, mth, code.registers);
        frame.args_start_reg = code.args_start_reg;
        let mut cfg = build_ssa(
            decoded,
            &code.tries,
            root,
            cls,
            mth,
            &frame,
            FALLBACK_LAST_STAGE,
            self.ctx.args.extract_finally,
        )?;
        apply_method_parameters(&mut cfg, root, cls, mth);
        apply_variable_names(&mut cfg, root);
        Ok(cfg)
    }
}

fn fallback_code(ctx: &GenContext<'_>, cls: usize, mth: usize) -> Result<FallbackCode, String> {
    let root = ctx.root;
    let m = &root.classes[cls].methods[mth];
    let off = m.code_offset.ok_or("no code")?;
    let code = ctx.method_code(cls, off)?;
    let mctx = MethodCtx {
        return_type: m.return_type.clone(),
        parent_class: root.classes[cls].info.clone(),
        root,
    };
    let decoded = InsnDecoder::new(code.resolver, &mctx)
        .process(&code.insns, code.units)
        .map_err(|e| e.to_string())?;
    let (insns, handlers) = fallback_decode(decoded, &code.tries, &root.ctx)?;
    Ok(FallbackCode { insns, handlers })
}

fn cached_source_line(
    ctx: &GenContext<'_>,
    cache: &RefCell<HashMap<(usize, usize), u32>>,
    cls: usize,
    mth: usize,
) -> u32 {
    if let Some(l) = cache.borrow().get(&(cls, mth)) {
        return *l;
    }
    let l = compute_source_line(ctx, cls, mth);
    cache.borrow_mut().insert((cls, mth), l);
    l
}

/// `DebugInfoAttachVisitor.setMethodSourceLine`
fn compute_source_line(ctx: &GenContext<'_>, cls: usize, mth: usize) -> u32 {
    match ctx.root.classes[cls].methods[mth].code_offset {
        Some(off) => ctx.method_source_line(cls, off),
        None => 0,
    }
}
