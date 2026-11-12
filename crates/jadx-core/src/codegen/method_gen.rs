//! `MethodGen`: method declarations (annotations, modifiers, generics, arguments, throws) and
//! method bodies.

use std::cell::RefCell;

use super::annotation_gen;
use super::class_gen::ClassGen;
use super::code_writer::CodeWriter;
use super::insn_gen::{label_name, Flags, InsnGen};
use super::name_gen::NameGen;
use super::region_gen::RegionGen;
use super::{CodegenError, CommentsLevel, DecompilationMode};
use crate::access::{
    AccessInfo, AccessKind, ACC_ABSTRACT, ACC_ANNOTATION, ACC_PUBLIC, ACC_VARARGS,
};
use crate::model::cfg::{ExceptionHandler, MethodCfg};
use crate::model::insn::{InsnKind, InsnNode, InsnType};
use crate::model::node::MethodNode;
use crate::model::types::ArgType;
use crate::passes::pipeline::FrameInfo;

type Res<T = ()> = Result<T, CodegenError>;

/// `MethodGen.needLabel`
fn need_label(insn: &InsnNode, prev: Option<&InsnNode>) -> bool {
    if insn.exc_handler.is_some() {
        return true;
    }
    if !insn.jumps.is_empty() {
        // don't add label for ifs else branch
        if let Some(p) = prev {
            if p.ty == InsnType::If && insn.jumps.len() == 1 {
                let j = &insn.jumps[0];
                if j.src == p.offset && j.dest == insn.offset {
                    if let InsnKind::If { target, .. } = &p.kind {
                        return insn.offset == *target;
                    }
                }
            }
        }
        return true;
    }
    false
}

/// `MethodGen`
pub struct MethodGen<'a> {
    cls_gen: &'a ClassGen<'a>,
    cfg: &'a MethodCfg,
    mth: usize,
    name_gen: RefCell<NameGen>,
}

/// Code state of a method without usable code: only `this` and the arguments.
pub fn no_code_cfg(root: &crate::model::node::Root, cls: usize, mth: usize) -> MethodCfg {
    let frame = FrameInfo::new(root, cls, mth, 0);
    let mut cfg = MethodCfg::default();
    crate::passes::ssa::init_arguments(
        &mut cfg,
        0,
        frame.is_static,
        &frame.this_type,
        &frame.arg_types,
    );
    let _ = crate::passes::init_code_vars::init_code_vars(&mut cfg);
    super::prepare::apply_method_parameters(&mut cfg, root, cls, mth);
    super::var_names::apply_variable_names(&mut cfg, root);
    cfg
}

impl<'a> MethodGen<'a> {
    /// Generator for method `mth` of the class of `cls_gen` with the code state `cfg`.
    pub fn new(cls_gen: &'a ClassGen<'a>, cfg: &'a MethodCfg, mth: usize) -> Self {
        let outer = cls_gen.outer_name_gen();
        let name_gen = NameGen::new(
            cls_gen.env(),
            cls_gen.class_index(),
            outer.as_ref(),
            cls_gen.is_fallback_mode(),
        );
        MethodGen {
            cls_gen,
            cfg,
            mth,
            name_gen: RefCell::new(name_gen),
        }
    }

    pub fn class_gen(&self) -> &'a ClassGen<'a> {
        self.cls_gen
    }

    pub fn cfg(&self) -> &'a MethodCfg {
        self.cfg
    }

    pub fn method_index(&self) -> usize {
        self.mth
    }

    pub fn method_node(&self) -> &'a MethodNode {
        &self.cls_gen.env().root.classes[self.cls_gen.class_index()].methods[self.mth]
    }

    pub fn name_gen(&self) -> &RefCell<NameGen> {
        &self.name_gen
    }

    /// Assign names to the declared method arguments (what the method declaration does).
    pub fn assign_arg_names(&self) {
        for reg in &self.cfg.arg_regs {
            if let Some(cv) = reg.svar.and_then(|v| self.cfg.svars[v].code_var) {
                self.name_gen.borrow_mut().assign_arg(self.cfg, cv);
            }
        }
    }

    /// `addDefinition`; returns false if the method has no name and arguments (anonymous class
    /// constructor).
    pub fn add_definition(&self, code: &mut CodeWriter) -> Res<bool> {
        let mth = self.method_node();
        let env = self.cls_gen.env();
        let cls = &env.root.classes[self.cls_gen.class_index()];
        if mth.info.is_class_init() {
            code.start_line();
            code.add("static");
            return Ok(true);
        }
        if mth.anonymous_constructor {
            // don't add method name and arguments
            code.start_line();
            return Ok(false);
        }
        self.add_override_annotation(code)?;
        annotation_gen::add_annotations(self.cls_gen, code, &mth.attrs.annotations)?;

        let cls_acc = AccessInfo::new(cls.access, AccessKind::Class);
        let mut ai = AccessInfo::new(mth.access, AccessKind::Method);
        // don't add 'abstract' and 'public' to methods in interface
        if cls_acc.is_interface() {
            ai = ai.remove(ACC_ABSTRACT).remove(ACC_PUBLIC);
        }
        // don't add 'public' for annotations
        if cls_acc.is_annotation() {
            ai = ai.remove(ACC_PUBLIC);
        }
        if mth.alias.as_deref().is_some_and(|a| a != mth.info.name) && !ai.is_constructor() {
            self.cls_gen.add_renamed_comment(code, &mth.info.name, None);
        }
        if self.cfg.is_inconsistent() && env.check_comments_level(CommentsLevel::Error) {
            code.start_line_str("/*");
            code.inc_indent();
            code.start_line_str("Code decompiled incorrectly, please refer to instructions dump.");
            if !env.show_inconsistent_code {
                code.start_line_str("To view partially-correct add '--show-bad-code' argument");
            }
            code.dec_indent();
            code.start_line_str("*/");
        }
        code.start_line();
        code.add(&ai.make_string(env.check_comments_level(CommentsLevel::Info)));
        let no_code = mth.code_offset.is_none();
        if cls_acc.is_interface()
            && !no_code
            && !AccessInfo::new(mth.access, AccessKind::Method).is_static()
        {
            // add 'default' for method with code in interface
            code.add("default ");
        }
        if self
            .cls_gen
            .add_generic_type_parameters(code, &mth.type_params, false)?
        {
            code.add(" ");
        }
        if ai.is_constructor() {
            // constructor
            code.add(&env.short_name(&cls.info));
        } else {
            self.cls_gen.use_type(code, &mth.return_type)?;
            code.add(" ");
            code.add(mth.alias.as_deref().unwrap_or(&mth.info.name));
        }
        code.add("(");
        self.add_method_arguments(code)?;
        code.add(")");

        let throws = self.throws();
        annotation_gen::add_throws(self.cls_gen, code, &throws)?;

        // add default value for annotation class
        if cls.access & ACC_ANNOTATION != 0 {
            if let Some(def) = &mth.attrs.annotation_default {
                code.add(" default ");
                annotation_gen::encode_value(self.cls_gen, code, Some(def))?;
            }
        }
        Ok(true)
    }

    /// `MethodNode.getThrows`
    fn throws(&self) -> Vec<ArgType> {
        self.method_node()
            .attrs
            .throws
            .iter()
            .map(|d| ArgType::parse(d).unwrap_or_else(|_| ArgType::object(d)))
            .collect()
    }

    fn add_override_annotation(&self, code: &mut CodeWriter) -> Res {
        let mth = self.method_node();
        if mth.override_set.is_none() || mth.override_is_base {
            return Ok(());
        }
        let env = self.cls_gen.env();
        code.start_line_str("@Override");
        if env.check_comments_level(CommentsLevel::Info) {
            code.add(" // ");
            let mut names = Vec::new();
            for d in &mth.override_decls {
                let ci = env.class_info(d)?;
                names.push(env.full_name(&ci));
            }
            code.add(&names.join(", "));
        }
        Ok(())
    }

    fn add_method_arguments(&self, code: &mut CodeWriter) -> Res {
        let mth = self.method_node();
        let cfg = self.cfg;
        let args = &cfg.arg_regs;
        let last_arg_num = args.len().saturating_sub(1);
        let mut first = true;
        for (arg_num, mth_arg) in args.iter().enumerate() {
            if mth
                .skip_args
                .as_ref()
                .is_some_and(|s| s.get(arg_num).copied().unwrap_or(false))
            {
                continue;
            }
            if first {
                first = false;
            } else {
                code.add(", ");
            }
            let cv = mth_arg.svar.and_then(|v| cfg.svars[v].code_var);
            // add argument annotation
            annotation_gen::add_for_parameter(
                self.cls_gen,
                code,
                &mth.attrs.param_annotations,
                arg_num,
            )?;
            let (is_final, var_type) = match cv {
                Some(cv) => (cfg.code_vars[cv].is_final, cfg.code_vars[cv].ty.clone()),
                None => (false, None),
            };
            if is_final {
                code.add("final ");
            }
            let arg_type = match var_type {
                Some(t) if t != ArgType::unknown_any() => t,
                // occur on decompilation errors
                _ => mth_arg.init_type.clone(),
            };
            if arg_num == last_arg_num && mth.access & ACC_VARARGS != 0 {
                // change last array argument to varargs
                if let Some(el) = arg_type.array_element().filter(|_| arg_type.is_array()) {
                    self.cls_gen.use_type(code, el)?;
                    code.add("...");
                } else {
                    self.cls_gen.use_type(code, &arg_type)?;
                }
            } else {
                self.cls_gen.use_type(code, &arg_type)?;
            }
            code.add(" ");
            let var_name = match cv {
                Some(cv) => self.name_gen.borrow_mut().assign_arg(cfg, cv),
                None => format!("r{}", mth_arg.reg_num),
            };
            code.add(&var_name);
        }
        Ok(())
    }

    /// `addInstructions`
    pub fn add_instructions(&self, code: &mut CodeWriter) -> Res {
        let env = self.cls_gen.env();
        match env.decompilation_mode {
            DecompilationMode::Auto => {
                if self.cls_gen.is_fallback_mode() || self.cfg.regions.method_region.is_none() {
                    self.dump_instructions(code);
                    Ok(())
                } else {
                    self.add_region_insns(code)
                }
            }
            DecompilationMode::Restructure => self.add_region_insns(code),
            DecompilationMode::Simple | DecompilationMode::Fallback => {
                self.dump_instructions(code);
                Ok(())
            }
        }
    }

    /// `addRegionInsns`
    pub fn add_region_insns(&self, code: &mut CodeWriter) -> Res {
        let Some(region) = self.cfg.regions.method_region else {
            self.dump_instructions(code);
            return Ok(());
        };
        let region_gen = RegionGen::new(self);
        let start = code.clone();
        match region_gen.make_region(code, crate::model::region::Container::Region(region)) {
            Ok(()) => Ok(()),
            Err(e) => {
                // generation of the method code failed
                *code = start;
                self.cls_gen.add_method_error(&e);
                self.add_errors(code, &e);
                self.dump_instructions(code);
                Ok(())
            }
        }
    }

    fn add_errors(&self, code: &mut CodeWriter, e: &CodegenError) {
        if self
            .cls_gen
            .env()
            .check_comments_level(CommentsLevel::Error)
        {
            code.start_line_str("/*  JADX ERROR: Method code generation error");
            code.add("\n        ").add(&e.msg).add("*/");
        }
    }

    /// `dumpInstructions`
    pub fn dump_instructions(&self, code: &mut CodeWriter) {
        let env = self.cls_gen.env();
        let mth = self.method_node();
        if env.check_comments_level(CommentsLevel::Error) {
            code.start_line_str("/*");
            self.add_fallback_method_code(code);
            code.start_line_str("*/");
        }
        let cls = &env.root.classes[self.cls_gen.class_index()];
        let args: Vec<String> = mth.info.arg_types.iter().map(|t| t.to_string()).collect();
        code.start_line_str("throw new UnsupportedOperationException(\"Method not decompiled: ")
            .add(&env.full_name(&cls.info))
            .add(".")
            .add(mth.alias.as_deref().unwrap_or(&mth.info.name))
            .add("(")
            .add(&args.join(", "))
            .add("):")
            .add(&mth.info.return_type.to_string())
            .add("\");");
    }

    /// `addFallbackMethodCode` (`COMMENTED_DUMP`): the original instructions of the method.
    fn add_fallback_method_code(&self, code: &mut CodeWriter) {
        let env = self.cls_gen.env();
        if env.comments_level != CommentsLevel::Debug && self.cfg.units_count > 200 {
            code.inc_indent();
            code.start_line_str(&format!(
                "Method dump skipped, instruction units count: {}",
                self.cfg.units_count
            ));
            code.start_line_str("To view this dump add '--comments-level debug' option");
            code.dec_indent();
            return;
        }
        let cidx = self.cls_gen.class_index();
        let Some(loader) = env.loader else {
            return;
        };
        let fallback = match loader.fallback_code(cidx, self.mth) {
            Ok(f) => f,
            Err(e) => {
                code.start_line_str(&format!("// Can't load method instructions: {e}"));
                return;
            }
        };
        let cls_gen = ClassGen::new_fallback(env, cidx);
        let fcfg = MethodCfg {
            insns: fallback.insns.iter().flatten().cloned().collect(),
            this_arg: self.cfg.this_arg.clone(),
            arg_regs: self.cfg.arg_regs.clone(),
            ..Default::default()
        };
        // instruction ids follow the order of the instruction array
        let mgen = MethodGen::new(&cls_gen, &fcfg, self.mth);
        let ig = InsnGen::new(&mgen, true);
        code.inc_indent();
        if let Some(this) = &fcfg.this_arg {
            code.start_line_str(&format!("r{} = this;", this.reg_num));
        }
        let start_indent = code.indent();
        let list: Vec<&InsnNode> = fallback.insns.iter().flatten().collect();
        let mut prev: Option<&InsnNode> = None;
        for (id, insn) in list.iter().enumerate() {
            if let Some(err) = &insn.error {
                code.start_line_str("// ").add(err);
                prev = Some(insn);
                continue;
            }
            if need_label(insn, prev) {
                code.dec_indent();
                code.start_line_str(&format!("{}:", label_name(insn.offset)));
                code.inc_indent();
            }
            if insn.ty == InsnType::Nop {
                prev = Some(insn);
                continue;
            }
            let escape_comment = insn.ty == InsnType::ConstStr
                && matches!(&insn.kind, InsnKind::ConstStr(u) if String::from_utf16_lossy(u).contains("*/"));
            if escape_comment {
                code.dec_indent();
                code.start_line_str("*/");
                code.start_line_str("//  ");
            } else {
                code.start_line();
            }
            if let Some(res) = &insn.result {
                if res.init_type.is_type_known() {
                    code.add(&res.init_type.to_string()).add(" ");
                }
            }
            match ig.make_insn(id, code, Some(Flags::Inline)) {
                Ok(()) => {
                    if escape_comment {
                        code.start_line_str("/*");
                        code.inc_indent();
                    }
                    self.add_catch_comment(code, insn, &fallback.handlers);
                }
                Err(_) => {
                    code.set_indent(start_indent);
                    code.start_line_str(&format!("// error: {}", crate::dump::dump_insn(insn)));
                }
            }
            prev = Some(insn);
        }
        code.dec_indent();
    }

    /// `addCatchComment` (raw mode)
    fn add_catch_comment(
        &self,
        code: &mut CodeWriter,
        insn: &InsnNode,
        handlers: &[ExceptionHandler],
    ) {
        let Some(catch) = &insn.catch else {
            return;
        };
        code.add("     // Catch:");
        for &h in catch {
            let handler = &handlers[h];
            code.add(" ");
            let _ = self.cls_gen.use_class(code, &handler.arg_type());
            code.add(" -> ").add(&label_name(handler.handler_offset));
        }
    }
}
