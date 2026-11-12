//! Class level pass driver: runs the method and class passes of the default pipeline over a class
//! tree the way the reference does, one pass after the other over all methods of the top level
//! class and its inner classes, with the dependencies of a class processed before the class.
//!
//! The driver owns the [`Root`] because class passes modify classes, fields and methods. The
//! method code is provided by a [`CodeSource`].

use std::collections::HashMap;

use jadx_input_api::TryData;

use crate::decode::DecodedMethod;
use crate::model::cfg::MethodCfg;
use crate::model::node::{MethodId, Root};
use crate::model::types::ArgType;
use crate::passes::constructor::MethodEnv;
use crate::passes::pipeline::{build_ssa, FrameInfo};

/// Instructions, exception table and register count of a method with code.
pub struct LoadedCode {
    pub decoded: DecodedMethod,
    pub tries: Vec<TryData>,
    pub regs_count: usize,
}

/// Provider of method code.
pub trait CodeSource {
    fn load_code(&self, root: &Root, cls: usize, mth: usize) -> Result<LoadedCode, String>;
}

impl<T: CodeSource + ?Sized> CodeSource for &T {
    fn load_code(&self, root: &Root, cls: usize, mth: usize) -> Result<LoadedCode, String> {
        (**self).load_code(root, cls, mth)
    }
}

/// Pass pipeline selection. Every pass has a stage number (see [`pass_stage`]); a pass runs when its
/// stage is not above `stage`. Passes keep the order of the reference pass list.
#[derive(Debug, Clone)]
pub struct DriverOptions {
    pub stage: u32,
    pub extract_finally: bool,
    /// `JadxArgs.isInlineMethods`
    pub inline_methods: bool,
    /// `JadxArgs.isRespectBytecodeAccModifiers`
    pub respect_bytecode_acc_modifiers: bool,
}

impl Default for DriverOptions {
    fn default() -> Self {
        DriverOptions {
            stage: u32::MAX,
            extract_finally: true,
            inline_methods: true,
            respect_bytecode_acc_modifiers: false,
        }
    }
}

/// A method pass: the method code, its environment and the state of all methods.
type MethodPassFn<'a> =
    dyn Fn(&mut MethodCfg, &MethodEnv, &HashMap<MethodId, MethodState>) -> Result<(), String> + 'a;

/// A class pass: the root, the state of all methods and the class index.
type ClassPassFn<'a> =
    dyn Fn(&mut Root, &mut HashMap<MethodId, MethodState>, usize) -> Result<(), String> + 'a;

/// State of a method during processing.
#[derive(Default)]
pub struct MethodState {
    pub cfg: Option<MethodCfg>,
    /// First error of a pass; the method is skipped by all later passes.
    pub failed: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClassPhase {
    NotLoaded,
    Started,
    Complete,
}

/// One element of the traversal of a class tree.
#[derive(Clone, Copy)]
enum Visit {
    Class(usize),
    Method(usize, usize),
}

pub struct ClassDriver<'s> {
    pub root: Root,
    src: Box<dyn CodeSource + Send + 's>,
    opts: DriverOptions,
    pub methods: HashMap<MethodId, MethodState>,
    phase: Vec<ClassPhase>,
    children: Vec<Vec<usize>>,
}

impl<'s> ClassDriver<'s> {
    pub fn new<S: CodeSource + Send + 's>(
        root: Root,
        src: S,
        opts: DriverOptions,
    ) -> ClassDriver<'s> {
        let n = root.classes.len();
        let mut sorted: Vec<usize> = (0..n).collect();
        sorted.sort_by_cached_key(|i| root.classes[*i].info.raw_name());
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
        for &i in &sorted {
            if let Some(p) = root.parent_index_of(&root.classes[i]) {
                if p != i {
                    children[p].push(i);
                }
            }
        }
        ClassDriver {
            root,
            src: Box::new(src),
            opts,
            methods: HashMap::new(),
            phase: vec![ClassPhase::NotLoaded; n],
            children,
        }
    }

    /// `ProcessState.isProcessComplete` of a top level class.
    pub fn is_process_complete(&self, top: usize) -> bool {
        self.phase[top] == ClassPhase::Complete
    }

    pub fn into_root(self) -> Root {
        self.root
    }

    /// Process every top level class like the code generation entry point does (without
    /// generating code): dependencies first, then the class.
    pub fn process_all(&mut self) {
        for ci in self.root.sorted_class_order() {
            if !self.root.classes[ci].is_inner() {
                self.process_with_deps(ci);
            }
        }
    }

    /// `ProcessClass.generateCode` without the code generation.
    pub fn process_with_deps(&mut self, top: usize) {
        if self.root.classes[top].dont_generate {
            self.process_class(top);
            return;
        }
        for dep in self.root.classes[top].dependencies.clone() {
            self.process_class(dep);
        }
        let codegen_deps = self.root.classes[top].codegen_deps.clone();
        if !codegen_deps.is_empty() {
            self.process_class(top);
            for dep in codegen_deps {
                self.process_class(dep);
            }
        }
        self.process_class(top);
    }

    /// `ProcessClass.process(cls, false)` of a top level class.
    pub fn process_class(&mut self, top: usize) {
        let top = self.root.top_parent_index(top);
        if self.phase[top] != ClassPhase::NotLoaded {
            return;
        }
        self.phase[top] = ClassPhase::Started;
        if std::env::var("LOG_ORDER").is_ok() {
            eprintln!("PROC {}", self.root.classes[top].info.raw_name());
        }
        let mut visits = Vec::new();
        self.collect_visits(top, &mut visits);
        self.load_methods(&visits);
        self.run_passes(&visits);
        self.phase[top] = ClassPhase::Complete;
    }

    fn collect_visits(&self, cls: usize, out: &mut Vec<Visit>) {
        out.push(Visit::Class(cls));
        for &inner in &self.children[cls] {
            self.collect_visits(inner, out);
        }
        for m in 0..self.root.classes[cls].methods.len() {
            out.push(Visit::Method(cls, m));
        }
    }

    /// Method loading and the passes up to the type inference.
    fn load_methods(&mut self, visits: &[Visit]) {
        for v in visits {
            let Visit::Method(c, m) = *v else {
                continue;
            };
            if self.root.classes[c].methods[m].code_offset.is_none() {
                continue;
            }
            let state = self.load_method(c, m);
            self.methods.insert((c, m), state);
        }
    }

    fn load_method(&mut self, c: usize, m: usize) -> MethodState {
        let loaded = match self.src.load_code(&self.root, c, m) {
            Ok(l) => l,
            Err(e) => {
                return MethodState {
                    cfg: None,
                    failed: Some(e),
                }
            }
        };
        let root = &self.root;
        let frame = FrameInfo::new(root, c, m, loaded.regs_count);
        let extract_finally = self.opts.extract_finally;
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            build_ssa(
                loaded.decoded,
                &loaded.tries,
                root,
                c,
                m,
                &frame,
                9,
                extract_finally,
            )
        }));
        match built {
            Ok(Ok(cfg)) => MethodState {
                cfg: Some(cfg),
                failed: None,
            },
            Ok(Err(e)) => MethodState {
                cfg: None,
                failed: Some(e),
            },
            Err(_) => MethodState {
                cfg: None,
                failed: Some("panic while loading".to_string()),
            },
        }
    }

    /// Run `f` for every loaded method of the traversal.
    fn method_pass(&mut self, visits: &[Visit], f: &MethodPassFn) {
        for v in visits {
            if let Visit::Method(c, m) = *v {
                self.method_pass_one(c, m, f);
            }
        }
    }

    /// Run `f` for one method if it is loaded and did not fail.
    fn method_pass_one(&mut self, c: usize, m: usize, f: &MethodPassFn) {
        let Some(st) = self.methods.get_mut(&(c, m)) else {
            return;
        };
        if st.failed.is_some() {
            return;
        }
        let Some(mut cfg) = st.cfg.take() else {
            return;
        };
        let env = MethodEnv {
            root: &self.root,
            cls: c,
            mth: m,
        };
        let methods = &self.methods;
        let r =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut cfg, &env, methods)));
        let st = self.methods.get_mut(&(c, m)).expect("method state");
        st.cfg = Some(cfg);
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => st.failed = Some(e),
            Err(_) => st.failed = Some("panic in pass".to_string()),
        }
    }

    /// Run `f` for every class of the traversal.
    fn class_pass(&mut self, visits: &[Visit], f: &ClassPassFn) {
        for v in visits {
            if let Visit::Class(c) = *v {
                // errors of class passes are reported as class warnings in the reference
                let _ = f(&mut self.root, &mut self.methods, c);
            }
        }
    }

    /// `MethodThrowsVisitor.visit`
    fn throws_visit(&mut self, id: MethodId) {
        use crate::passes::method_throws as mt;
        let (c, m) = id;
        if self.root.classes[c].methods[m].throws_attr.is_none() {
            self.root.classes[c].methods[m].throws_attr = Some(Default::default());
        }
        let visited = self.root.classes[c].methods[m]
            .throws_attr
            .as_ref()
            .is_some_and(|a| a.visited);
        let no_code = self.root.classes[c].methods[m].code_offset.is_none();
        let has_blocks = self.methods.get(&id).is_some_and(|s| s.cfg.is_some());
        if !visited && (no_code || has_blocks) {
            if let Some(a) = self.root.classes[c].methods[m].throws_attr.as_mut() {
                a.visited = true;
            }
            self.throws_process(id);
        }
        let declared = self.root.classes[c].methods[m].attrs.throws.clone();
        let mut add = Vec::new();
        for d in &declared {
            let ty = ArgType::object(d);
            if mt::validate_exception(&self.root, &ty) != mt::ExceptionType::NoException {
                if let Some(n) = ty.object_name() {
                    add.push(n);
                }
            }
        }
        let mut attr = self.root.classes[c].methods[m]
            .throws_attr
            .take()
            .unwrap_or_default();
        for n in &add {
            attr.add(n);
        }
        mt::merge_exceptions(&self.root, &mut attr);
        self.root.classes[c].methods[m].throws_attr = Some(attr);
    }

    fn throws_process(&mut self, id: MethodId) {
        use crate::passes::method_throws as mt;
        let plan = match self.methods.get(&id).and_then(|s| s.cfg.as_ref()) {
            Some(cfg) => mt::plan(&self.root, cfg),
            None => return,
        };
        for p in plan {
            match p.step {
                mt::Step::Throw(ty) => self.visit_throws(id, &ty, &p.excluded),
                mt::Step::CallApp(callee) => {
                    self.throws_visit(callee);
                    let list = self.root.classes[callee.0].methods[callee.1]
                        .throws_attr
                        .as_ref()
                        .filter(|a| !a.is_empty())
                        .map(|a| a.java_order())
                        .unwrap_or_default();
                    for s in list {
                        self.visit_throws(id, &ArgType::object(&s), &p.excluded);
                    }
                }
                mt::Step::CallClsp(types) => {
                    for t in types {
                        self.visit_throws(id, &t, &p.excluded);
                    }
                }
            }
        }
    }

    /// `visitThrows`
    fn visit_throws(&mut self, id: MethodId, ty: &ArgType, excluded: &[String]) {
        use crate::passes::method_throws as mt;
        if !ty.is_type_known() {
            return;
        }
        // an unknown type hierarchy is assumed to be an exception
        let required = match mt::validate_exception(&self.root, ty) {
            mt::ExceptionType::UnknownType | mt::ExceptionType::ThrowsRequired => true,
            mt::ExceptionType::Runtime | mt::ExceptionType::NoException => false,
        };
        if !required {
            return;
        }
        let Some(name) = ty.object_name() else {
            return;
        };
        if excluded
            .iter()
            .any(|e| mt::is_base_exception(&self.root, &name, e))
        {
            return;
        }
        if let Some(a) = self.root.classes[id.0].methods[id.1].throws_attr.as_mut() {
            a.add(&name);
        }
    }

    /// `MarkMethodsForInline.process`: set the inline attribute of a method if it has none.
    /// `own` is the code of the method if it is currently taken out of the state. Returns false
    /// if the method can't be analyzed because it is not loaded.
    fn mark_process(&mut self, id: MethodId, own: Option<&MethodCfg>) -> bool {
        if self.root.classes[id.0].methods[id.1].inline.is_some() {
            return true;
        }
        let cfg = own.or_else(|| self.methods.get(&id).and_then(|s| s.cfg.as_ref()));
        let respect = self.opts.respect_bytecode_acc_modifiers;
        match crate::passes::mark_methods_for_inline::process(&self.root, id, cfg, respect) {
            Some(marked) => {
                crate::passes::mark_methods_for_inline::apply_visibility(
                    &mut self.root,
                    &marked.make_public,
                );
                self.root.classes[id.0].methods[id.1].inline = Some(marked.attr);
                true
            }
            None => false,
        }
    }

    /// `InlineMethods`
    fn inline_methods(&mut self, visits: &[Visit]) {
        for v in visits {
            let Visit::Method(c, m) = *v else {
                continue;
            };
            let Some(st) = self.methods.get_mut(&(c, m)) else {
                continue;
            };
            if st.failed.is_some() {
                continue;
            }
            let Some(mut cfg) = st.cfg.take() else {
                continue;
            };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.inline_in_method((c, m), &mut cfg)
            }));
            let st = self.methods.get_mut(&(c, m)).expect("method state");
            st.cfg = Some(cfg);
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => st.failed = Some(e),
                Err(_) => st.failed = Some("panic in pass".to_string()),
            }
        }
    }

    fn inline_in_method(&mut self, mth: MethodId, cfg: &mut MethodCfg) -> Result<(), String> {
        use crate::model::insn::InsnType;
        for block in cfg.blocks.clone() {
            for insn in cfg.block(block).insns.clone() {
                if cfg.insn(insn).ty != InsnType::Invoke {
                    continue;
                }
                let Some(details) = cfg.insn(insn).details.clone() else {
                    continue;
                };
                let Some(callee) = self.root.resolve_method(&details) else {
                    continue;
                };
                self.process_invoke_for_inline(mth, cfg, block, insn, callee)?;
            }
        }
        Ok(())
    }

    fn process_invoke_for_inline(
        &mut self,
        mth: MethodId,
        cfg: &mut MethodCfg,
        block: crate::model::insn::BlockId,
        insn: crate::model::insn::InsnId,
        callee: MethodId,
    ) -> Result<(), String> {
        let own = if callee == mth { Some(&*cfg) } else { None };
        let mut loaded = self.mark_process(callee, own);
        if !loaded {
            // method is not yet loaded => force process
            let top = self.root.top_parent_index(callee.0);
            // the code of the method in progress is visible to the processing of other classes
            if let Some(st) = self.methods.get_mut(&mth) {
                st.cfg = Some(cfg.clone());
            }
            self.process_class(top);
            if let Some(st) = self.methods.get_mut(&mth) {
                st.cfg = None;
            }
            let own = if callee == mth { Some(&*cfg) } else { None };
            loaded = self.mark_process(callee, own);
            if !loaded {
                let name = format!(
                    "{}.{}",
                    self.root.classes[callee.0].info.full_name(),
                    self.root.classes[callee.0].methods[callee.1]
                        .info
                        .short_id()
                );
                cfg.warnings.push(format!(
                    "Failed to check method for inline after forced process{name}"
                ));
                return Ok(());
            }
        }
        let Some(attr) = self.root.classes[callee.0].methods[callee.1].inline.clone() else {
            return Ok(());
        };
        if attr.is_not_needed() {
            return Ok(());
        }
        let (done, updates) = crate::passes::inline_methods::inline_method(
            &self.root, cfg, callee, &attr, block, insn,
        );
        if let Some(stored) = self.root.classes[callee.0].methods[callee.1]
            .inline
            .as_mut()
        {
            for (i, k, arg) in updates {
                stored.insns[i].args[k] = arg;
            }
        }
        if done {
            crate::passes::inline_methods::update_usage_info(&mut self.root, mth, callee, &attr);
        }
        Ok(())
    }

    /// `FixSwitchOverEnum`: a pass with a class and a method part, in traversal order.
    fn fix_switch_over_enum(&mut self, visits: &[Visit]) {
        use crate::passes::fix_switch_over_enum as fse;
        for v in visits {
            match *v {
                Visit::Class(c) => fse::init_cls_enum_map(&mut self.root, &self.methods, c),
                Visit::Method(c, m) => {
                    let pending = std::cell::RefCell::new(Vec::new());
                    self.method_pass_one(c, m, &|cfg, env, _| {
                        fse::fix_switch_over_enum(cfg, env, &mut pending.borrow_mut())
                    });
                    for field in pending.into_inner() {
                        fse::hide_enum_map_field(&mut self.root, field);
                    }
                }
            }
        }
    }

    /// Like [`ClassDriver::class_pass`] for passes that process the inner classes of a class
    /// before the class itself.
    fn class_pass_post(&mut self, top: usize, f: &ClassPassFn) {
        fn post(children: &[Vec<usize>], c: usize, out: &mut Vec<usize>) {
            for &i in &children[c] {
                post(children, i, out);
            }
            out.push(c);
        }
        let mut order = Vec::new();
        post(&self.children, top, &mut order);
        for c in order {
            if let Err(e) = f(&mut self.root, &mut self.methods, c) {
                if std::env::var("JADX_RS_ERRORS").is_ok() {
                    eprintln!("CLASS ERROR {}: {e}", self.root.classes[c].info.raw_name());
                }
            }
        }
    }

    fn run_passes(&mut self, visits: &[Visit]) {
        use crate::passes::regions as rg;
        let stage = self.opts.stage;
        let on = |s: u32| stage >= s;

        if on(pass_stage::MOD_VISITOR) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::debug_info::apply_method_parameters(cfg, env.root, env.cls, env.mth);
                Ok(())
            });
        }
        if on(pass_stage::KOTLIN_INTERNALS) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::kotlin_internals::process_kotlin_internals(cfg, env.root)
            });
        }
        if on(pass_stage::INLINE_METHODS) && self.opts.inline_methods {
            self.inline_methods(visits);
        }
        if on(pass_stage::GENERIC_TYPES) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::generic_types::generic_types(cfg, env.root)
            });
        }
        if on(pass_stage::SHADOW_FIELD) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::shadow_field::shadow_field(cfg, env.root)
            });
        }
        if on(pass_stage::DEBOXING) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::deboxing::deboxing(cfg, env)
            });
        }
        if on(pass_stage::ANONYMOUS_CLASS) {
            self.class_pass(visits, &|root, methods, c| {
                crate::passes::anonymous_class::anonymous_class_visitor(root, methods, c)
            });
        }
        if on(pass_stage::MOD_VISITOR) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::mod_visitor::mod_visitor(cfg, env)
            });
        }
        if on(pass_stage::CODE_SHRINK) {
            self.method_pass(visits, &|cfg, _, _| {
                crate::passes::code_shrink::code_shrink(cfg)
            });
        }
        if on(pass_stage::REPLACE_NEW_ARRAY) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::replace_new_array::replace_new_array(cfg, env)
            });
        }
        if on(pass_stage::REGION_MAKER) {
            self.method_pass(visits, &|cfg, env, _| rg::region_maker_visitor(cfg, env));
        }
        if on(pass_stage::IF_REGION) {
            self.method_pass(visits, &|cfg, env, _| {
                if cfg.blocks.is_empty() {
                    return Ok(());
                }
                rg::if_visitor::if_region_visitor(cfg, env)
            });
        }
        if on(pass_stage::SWITCH_OVER_STRING) {
            self.method_pass(visits, &|cfg, env, _| {
                if cfg.blocks.is_empty() {
                    return Ok(());
                }
                rg::switch_over_string::switch_over_string(cfg, env)
            });
        }
        if on(pass_stage::RETURN) {
            self.method_pass(visits, &|cfg, _, _| rg::if_visitor::return_visitor(cfg));
        }
        if on(pass_stage::CLEAN_REGIONS) {
            self.method_pass(visits, &|cfg, _, _| rg::clean::clean_regions(cfg));
        }
        if on(pass_stage::METHOD_THROWS) {
            for v in visits {
                if let Visit::Method(c, m) = *v {
                    if self
                        .methods
                        .get(&(c, m))
                        .is_some_and(|s| s.failed.is_some())
                    {
                        continue;
                    }
                    self.throws_visit((c, m));
                }
            }
        }
        if on(pass_stage::CODE_SHRINK2) {
            self.method_pass(visits, &|cfg, _, _| rg::shrink_method(cfg));
        }
        if on(pass_stage::METHOD_INVOKE) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::method_invoke::method_invoke(cfg, env)
            });
        }
        if on(pass_stage::SIMPLIFY) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::simplify::simplify(cfg, env)
            });
        }
        if on(pass_stage::CHECK_REGIONS) {
            self.method_pass(visits, &|cfg, _, _| rg::check::check_regions(cfg));
        }
        if on(pass_stage::FIX_INSTRUCTIONS) {
            self.method_pass(visits, &|cfg, _, _| {
                crate::passes::fix_instructions::fix_instructions(cfg)
            });
        }
        if on(pass_stage::ENUM_VISITOR) {
            self.class_pass(visits, &|root, methods, c| {
                crate::passes::enum_visitor::enum_visitor(root, methods, c)
            });
        }
        if on(pass_stage::FIX_SWITCH_OVER_ENUM) {
            self.fix_switch_over_enum(visits);
        }
        if on(pass_stage::FIX_ACCESS_MODIFIERS) && !self.opts.respect_bytecode_acc_modifiers {
            for v in visits {
                match *v {
                    Visit::Class(c) => {
                        crate::passes::fix_access_modifiers::fix_class_visibility(&mut self.root, c)
                    }
                    Visit::Method(c, m) => {
                        crate::passes::fix_access_modifiers::fix_method_visibility(
                            &mut self.root,
                            (c, m),
                        )
                    }
                }
            }
        }
        if on(pass_stage::EXTRACT_FIELD_INIT) {
            if let Some(Visit::Class(top)) = visits.first().copied() {
                self.class_pass_post(top, &|root, methods, c| {
                    crate::passes::extract_field_init::extract_field_init(root, methods, c)
                });
            }
        }
        if on(pass_stage::CLASS_MODIFIER) {
            if let Some(Visit::Class(top)) = visits.first().copied() {
                self.class_pass_post(top, &|root, methods, c| {
                    crate::passes::class_modifier::class_modifier(root, methods, c)
                });
            }
        }
        if on(pass_stage::LOOP_REGION) {
            self.method_pass(visits, &|cfg, env, _| {
                rg::loop_visitor::loop_region_visitor(cfg, env)
            });
        }
        if on(pass_stage::SWITCH_BREAK) {
            self.method_pass(visits, &|cfg, env, _| {
                rg::switch_break::switch_break_visitor(cfg, env)
            });
        }
        if on(pass_stage::MARK_METHODS_FOR_INLINE) && self.opts.inline_methods {
            for v in visits {
                if let Visit::Method(c, m) = *v {
                    if self
                        .methods
                        .get(&(c, m))
                        .is_some_and(|s| s.failed.is_some())
                    {
                        continue;
                    }
                    self.mark_process((c, m), None);
                }
            }
        }
        if on(pass_stage::PROCESS_VARIABLES) {
            self.method_pass(visits, &|cfg, env, methods| {
                crate::passes::process_variables::process_variables(cfg, env, methods)
            });
        }
        if on(pass_stage::APPLY_VARIABLE_NAMES) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::codegen::var_names::apply_variable_names(cfg, env.root);
                Ok(())
            });
        }
        if on(pass_stage::PREPARE_FOR_CODEGEN) {
            self.method_pass(visits, &|cfg, env, _| {
                crate::passes::prepare_for_codegen::prepare_for_codegen(cfg, env)
            });
        }
    }
}

/// Stage numbers of the passes: 10 to 19 are the passes ported first (see `build_ssa`), later
/// numbers were assigned in the order the passes were ported. The oracle tools
/// (`tests/oracle/java/PipelineDump.java`) use the same numbers.
pub mod pass_stage {
    pub const MOD_VISITOR: u32 = 10;
    pub const CODE_SHRINK: u32 = 11;
    pub const REGION_MAKER: u32 = 12;
    pub const IF_REGION: u32 = 13;
    pub const RETURN: u32 = 14;
    pub const CLEAN_REGIONS: u32 = 15;
    pub const CODE_SHRINK2: u32 = 16;
    pub const CHECK_REGIONS: u32 = 17;
    pub const LOOP_REGION: u32 = 18;
    pub const SWITCH_BREAK: u32 = 19;
    pub const REPLACE_NEW_ARRAY: u32 = 20;
    pub const DEBOXING: u32 = 21;
    pub const SHADOW_FIELD: u32 = 22;
    pub const GENERIC_TYPES: u32 = 23;
    pub const KOTLIN_INTERNALS: u32 = 24;
    pub const SWITCH_OVER_STRING: u32 = 25;
    pub const METHOD_THROWS: u32 = 26;
    pub const METHOD_INVOKE: u32 = 27;
    pub const SIMPLIFY: u32 = 28;
    pub const FIX_INSTRUCTIONS: u32 = 29;
    pub const PROCESS_VARIABLES: u32 = 30;
    pub const APPLY_VARIABLE_NAMES: u32 = 31;
    pub const PREPARE_FOR_CODEGEN: u32 = 32;
    pub const ANONYMOUS_CLASS: u32 = 33;
    pub const EXTRACT_FIELD_INIT: u32 = 34;
    pub const CLASS_MODIFIER: u32 = 35;
    pub const ENUM_VISITOR: u32 = 36;
    pub const FIX_SWITCH_OVER_ENUM: u32 = 37;
    pub const INLINE_METHODS: u32 = 38;
    pub const FIX_ACCESS_MODIFIERS: u32 = 40;
    pub const MARK_METHODS_FOR_INLINE: u32 = 41;
}
