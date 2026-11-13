//! Instruction generator (`InsnGen`, with `ConditionGen` for ternary conditions): code for one
//! instruction or one wrapped argument expression.

use super::class_gen::ClassGen;
use super::code_writer::CodeWriter;
use super::method_gen::MethodGen;
use super::name_gen::{SUPER_ARG_NAME, THIS_ARG_NAME};
use super::type_gen::literal_to_string;
use super::{CodegenError, GenEnv};
use crate::model::cfg::MethodCfg;
use crate::model::condition::{CondMode, IfCondition};
use crate::model::info::{ClassInfo, FieldInfo, MethodInfo};
use crate::model::insn::{
    iflag, ArithOp, CallType, IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType,
    InvokeType, LiteralArg, RegisterArg, WrapArg,
};
use crate::model::node::{FieldReplace, MethodNode, Root};
use crate::model::types::{ArgType, PrimitiveType};

type Res<T = ()> = Result<T, CodegenError>;

/// `InsnGen.Flags`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flags {
    /// Only the instruction body, wrapped in brackets where the operator precedence needs it.
    BodyOnly,
    /// Only the instruction body, without brackets and without brackets around negative literals.
    BodyOnlyNoWrap,
    /// Statement without line start and terminator.
    Inline,
}

/// The `Set<Flags>` passed down while generating a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Empty,
    BodyOnly,
    BodyOnlyNoWrap,
}

/// Display name for each `FieldReplace::Var` field of `cls`, by field index: the name already
/// assigned (and de-duplicated) to the corresponding argument of the constructor-call instruction
/// `insn`, read via `name_gen`/`cfg` (the code state `insn` itself lives in). See `instance_field`.
fn build_field_replace_names(
    root: &Root,
    cls: usize,
    insn: &InsnNode,
    name_gen: &super::name_gen::NameGen,
    cfg: &MethodCfg,
) -> std::collections::HashMap<usize, String> {
    let mut names = std::collections::HashMap::new();
    for (fi, field) in root.classes[cls].fields.iter().enumerate() {
        if let Some(FieldReplace::Var { arg: arg_idx, .. }) = &field.replace {
            if let Some(InsnArg::Register(reg)) = insn.args.get(*arg_idx) {
                names.insert(fi, name_gen.use_arg(cfg, reg));
            }
        }
    }
    names
}

/// `InsnGen`: the generator works on the finished code state of one method.
pub struct InsnGen<'m, 'a> {
    mgen: &'m MethodGen<'a>,
    fallback: bool,
}

impl<'m, 'a> InsnGen<'m, 'a> {
    pub fn new(mgen: &'m MethodGen<'a>, fallback: bool) -> Self {
        InsnGen { mgen, fallback }
    }

    pub(super) fn cfg(&self) -> &'a MethodCfg {
        self.mgen.cfg()
    }

    pub(super) fn mgen(&self) -> &'m MethodGen<'a> {
        self.mgen
    }

    pub(super) fn is_fallback(&self) -> bool {
        self.fallback
    }

    pub(super) fn env(&self) -> &'a GenEnv<'a> {
        self.mgen.class_gen().env()
    }

    fn root(&self) -> &'a Root {
        self.env().root
    }

    pub(super) fn cls_gen(&self) -> &'a ClassGen<'a> {
        self.mgen.class_gen()
    }

    pub(super) fn insn(&self, id: InsnId) -> &'a InsnNode {
        &self.cfg().insns[id]
    }

    /// `InsnArg.getType`
    pub(super) fn arg_type(&self, arg: &InsnArg) -> ArgType {
        match arg {
            InsnArg::Register(r) => match r.svar {
                Some(v) => self.cfg().svars[v].ty.clone(),
                None => r.init_type.clone(),
            },
            InsnArg::Literal(l) => l.ty.clone(),
            InsnArg::Wrapped(w) => w.ty.clone(),
            InsnArg::Named(n) => n.ty.clone(),
        }
    }

    pub(super) fn use_class_info(&self, code: &mut CodeWriter, ci: &ClassInfo) -> Res {
        self.cls_gen().use_class_info(code, ci)
    }

    pub(super) fn use_type(&self, code: &mut CodeWriter, ty: &ArgType) -> Res {
        self.cls_gen().use_type(code, ty)
    }

    // ---- arguments -------------------------------------------------------------------------

    /// `addArgDot`
    pub fn add_arg_dot(&self, code: &mut CodeWriter, arg: &InsnArg) -> Res {
        let len = code.len();
        self.add_arg(code, arg, true)?;
        if len != code.len() {
            code.add(".");
        }
        Ok(())
    }

    /// `addArg(code, arg, wrap)`
    pub fn add_arg(&self, code: &mut CodeWriter, arg: &InsnArg, wrap: bool) -> Res {
        self.add_arg_state(
            code,
            arg,
            if wrap {
                State::BodyOnly
            } else {
                State::BodyOnlyNoWrap
            },
        )
    }

    fn add_arg_state(&self, code: &mut CodeWriter, arg: &InsnArg, state: State) -> Res {
        match arg {
            InsnArg::Register(reg) => {
                self.add_register(code, reg);
                Ok(())
            }
            InsnArg::Literal(l) => self.add_literal_arg(code, l, state),
            InsnArg::Wrapped(w) => self.add_wrapped_arg(code, w, state),
            InsnArg::Named(n) => {
                // The handler's catch clause (`make_catch_block`) is the single place that
                // assigns and deduplicates this named argument's display name (`NamedArg.setName`
                // on the Java side, mutating the one object shared with every `Move` that reads
                // it); resolve through that same assignment here instead of printing the raw,
                // never-deduplicated base name, which would (and did) make an unrelated variable
                // of the same base name print identically to this one.
                let resolved = n
                    .handler
                    .and_then(|h| self.mgen.name_gen().borrow().named_arg_name(h));
                code.add(resolved.as_deref().unwrap_or(&n.name));
                Ok(())
            }
        }
    }

    fn add_register(&self, code: &mut CodeWriter, reg: &RegisterArg) {
        let name = self.mgen.name_gen().borrow().use_arg(self.cfg(), reg);
        code.add(&name);
    }

    fn add_literal_arg(&self, code: &mut CodeWriter, lit: &LiteralArg, state: State) -> Res {
        let s = self.lit(lit)?;
        if state != State::BodyOnlyNoWrap && s.starts_with('-') {
            code.add("(").add(&s).add(")");
        } else {
            code.add(&s);
        }
        Ok(())
    }

    fn add_wrapped_arg(&self, code: &mut CodeWriter, w: &WrapArg, state: State) -> Res {
        let insn = self.insn(w.insn);
        if insn.contains_flag(iflag::FORCE_ASSIGN_INLINE) {
            code.add("(");
            self.make_insn(w.insn, code, Some(Flags::Inline))?;
            code.add(")");
            Ok(())
        } else {
            self.make_insn_body(code, w.insn, state)
        }
    }

    /// `assignVar`: result of the instruction, declared or not.
    pub fn assign_var(&self, code: &mut CodeWriter, insn: &InsnNode) -> Res {
        let Some(res) = &insn.result else {
            return Ok(());
        };
        if insn.contains_flag(iflag::DECLARE_VAR) {
            self.declare_var(code, res)
        } else {
            self.add_register(code, res);
            Ok(())
        }
    }

    /// `declareVar`
    pub fn declare_var(&self, code: &mut CodeWriter, reg: &RegisterArg) -> Res {
        let cv = reg
            .svar
            .and_then(|v| self.cfg().svars[v].code_var)
            .ok_or_else(|| CodegenError::new("Code variable not set"))?;
        self.declare_code_var(code, cv)
    }

    /// `declareVar(ICodeWriter, CodeVar)`
    pub fn declare_code_var(&self, code: &mut CodeWriter, cv: crate::model::ssa::CodeVarId) -> Res {
        let cfg = self.cfg();
        let var = &cfg.code_vars[cv];
        if var.is_final {
            code.add("final ");
        }
        let ty = var
            .ty
            .as_ref()
            .ok_or_else(|| CodegenError::new("Code variable type not set"))?;
        self.use_type(code, ty)?;
        code.add(" ");
        let name =
            self.mgen
                .name_gen()
                .borrow_mut()
                .assign_arg(self.mgen.class_gen().env(), cfg, cv);
        code.add(&name);
        Ok(())
    }

    fn lit(&self, arg: &LiteralArg) -> Res<String> {
        literal_to_string(
            arg.literal,
            &arg.ty,
            &self.env().string_utils,
            self.fallback,
            arg.explicit_primitive,
        )
    }

    // ---- fields ----------------------------------------------------------------------------

    /// `instanceField`: a field marked for replacement (`FieldReplace`, set by `AnonymousClassVisitor`
    /// / `ClassModifier` for captured variables and the outer instance reference) is printed as the
    /// captured variable or `Outer.this` instead of the (removed) field itself.
    fn instance_field(&self, code: &mut CodeWriter, field: &FieldInfo, arg: &InsnArg) -> Res {
        if let Some((cls, fi)) = self.root().resolve_field_id(field) {
            // `FieldReplace` is only ever set on a field by its own declaring class; never honour
            // it when `resolve_field_id` fell back to an inherited field of the same name/type
            // (`deep_resolve_field`), which is a different field than the one actually accessed.
            let f = &self.root().classes[cls].fields[fi];
            let replace = if f.info == *field {
                f.replace.as_ref()
            } else {
                None
            };
            match replace {
                Some(FieldReplace::ClassInstance(info)) => {
                    self.use_class_info(code, info)?;
                    code.add(".this");
                    return Ok(());
                }
                Some(FieldReplace::Var { ctor, arg: arg_idx }) => {
                    // Resolved once, when the class body was inlined (`build_field_replace_names`):
                    // the name of the captured variable as already assigned at the single
                    // constructor-call site, i.e. the *caller's* name, not a name independently
                    // re-derived (and re-deduplicated against the wrong scope) from the
                    // constructor's own, unrelated `CodeVarId` space. Two captured variables of
                    // the same guessed base name (e.g. two `boolean`s, both guessed "z") must
                    // keep the distinct, already-deduplicated names the caller gave them ("z",
                    // "z2"), not collide on a freshly-recomputed base name.
                    if let Some(name) = self.cls_gen().field_replace_name(fi) {
                        code.add(&name);
                        return Ok(());
                    }
                    // Fallback for a `FieldReplace::Var` field read outside an inlined class body
                    // (not expected on the normal inlining paths, kept for robustness): read the
                    // name straight off the constructor's own `CodeVar`.
                    let loaded = self.env().method_cfg(cls, *ctor);
                    if let Ok(ctor_cfg) = loaded.as_ref() {
                        if let Some(reg) = ctor_cfg.arg_regs.get(*arg_idx) {
                            let name = if reg.is_super {
                                SUPER_ARG_NAME.to_string()
                            } else if reg.is_this {
                                THIS_ARG_NAME.to_string()
                            } else {
                                reg.svar
                                    .and_then(|sv| ctor_cfg.svars[sv].code_var)
                                    .and_then(|cv| ctor_cfg.code_vars[cv].name.clone())
                                    .unwrap_or_else(|| format!("r{}", reg.reg_num))
                            };
                            code.add(&name);
                            return Ok(());
                        }
                    }
                    return Err(CodegenError::new(
                        "Missing constructor argument for the replaced field",
                    ));
                }
                None => {}
            }
        }
        self.add_arg_dot(code, arg)?;
        code.add(&self.root().field_name(field));
        Ok(())
    }

    /// `staticField`: a field marked `InlineType::InstanceField` (a singleton instance of an
    /// inlinable anonymous class, held in a `public static final` field initialized in its own
    /// class's initializer) is printed as the anonymous class body at its first/only use instead
    /// of a plain static field read.
    fn static_field(&self, code: &mut CodeWriter, field: &FieldInfo) -> Res {
        if let Some((fc, fi)) = self.root().resolve_field_id(field) {
            let field_node = &self.root().classes[fc].fields[fi];
            // Only act on an exact field match: `resolve_field_id` can fall back to an inherited
            // field of the same name/type (`deep_resolve_field`), which is a different field than
            // the one actually accessed and must not be inlined here.
            if field_node.info == *field
                && field_node.inline_instance_field
                && self.root().classes[fc].anonymous.is_some()
            {
                if let Some(init) = field_node.init_insn {
                    let loaded = self.env().method_cfg(fc, init.mth);
                    if let Ok(field_cfg) = loaded.as_ref() {
                        let insn = &field_cfg.insns[init.insn];
                        if insn.ty == InsnType::Constructor {
                            return self.inline_instance_field_constructor(
                                code, fc, field_cfg, init.mth, insn,
                            );
                        }
                    }
                }
            }
        }
        make_static_field_access(code, field, self.cls_gen())
    }

    /// `InsnGen.staticField`'s inline branch: the constructor call lives in the field's own
    /// initializer code (`FieldInitInsn`), a different method than the one currently being
    /// generated, so argument names are resolved against that method's own code state (`NameGen`
    /// keyed by its own SSA vars) instead of the current one; everything else (recursion check,
    /// base type, trailing class body) uses the current generator, same as
    /// `inline_anonymous_constructor`.
    fn inline_instance_field_constructor(
        &self,
        code: &mut CodeWriter,
        cls: usize,
        field_cfg: &MethodCfg,
        field_mth: usize,
        insn: &InsnNode,
    ) -> Res {
        if self.cls_gen().class_index() == cls {
            return Err(CodegenError::new(format!(
                "Anonymous inner class unlimited recursion detected. Convert class to inner: {}",
                self.root().classes[cls].info.full_name()
            )));
        }
        // The singleton field's own initializer can itself read another inlined singleton field
        // a few levels up; without tracking every class currently being inlined (not just the
        // immediate one, which the check above covers), such a cycle would recurse without bound.
        let _guard = self.env().enter_inlining(cls).map_err(CodegenError::new)?;
        let root = self.root();
        let Some(anon) = &root.classes[cls].anonymous else {
            return Err(CodegenError::new("Missing anonymous class attribute"));
        };
        let InsnKind::Constructor { call_mth, .. } = &insn.kind else {
            return Err(CodegenError::new("Missing constructor data"));
        };
        code.add("new ");
        self.use_type(code, &anon.base_type)?;
        let call_node = root.resolve_method(call_mth);
        let call_mth_node = call_node.map(|(c, m)| &root.classes[c].methods[m]);
        let field_cls_gen = ClassGen::new(self.env(), cls);
        let field_mgen = MethodGen::new(&field_cls_gen, field_cfg, field_mth);
        let field_ig = InsnGen::new(&field_mgen, self.fallback);
        field_ig.generate_method_arguments(code, insn, 0, call_mth_node)?;
        code.add(" ");

        let cls_gen = ClassGen::with_parent(self.env(), cls, self.cls_gen().parent_gen());
        cls_gen.set_outer_name_gen(self.mgen.name_gen().borrow().clone());
        // The constructor call lives in the field initializer's own code (`field_cfg`/`field_mgen`
        // above), not in the method currently being generated: resolve the captured arguments'
        // names against that code state, matching `generate_method_arguments` just above.
        cls_gen.set_field_replace_names(build_field_replace_names(
            root,
            cls,
            insn,
            &field_mgen.name_gen().borrow(),
            field_cfg,
        ));
        cls_gen.set_inlined(true);
        cls_gen.add_class_body(code, true)
    }

    // ---- instructions ----------------------------------------------------------------------

    /// `makeInsn`: `flag` `None` generates a full statement.
    pub fn make_insn(&self, id: InsnId, code: &mut CodeWriter, flag: Option<Flags>) -> Res {
        let insn = self.insn(id);
        if insn.ty == InsnType::RegionArg {
            return Ok(());
        }
        match flag {
            Some(Flags::BodyOnly) => self.make_insn_body(code, id, State::BodyOnly),
            Some(Flags::BodyOnlyNoWrap) => self.make_insn_body(code, id, State::BodyOnlyNoWrap),
            _ => {
                if flag != Some(Flags::Inline) {
                    code.start_line();
                    code.attach_insn_offset(insn.offset);
                }
                if let Some(res) = &insn.result {
                    let use_count = res.svar.map(|v| self.cfg().svars[v].use_count());
                    if use_count != Some(0) || insn.ty != InsnType::Constructor {
                        self.assign_var(code, insn)?;
                        code.add(" = ");
                    }
                }
                self.make_insn_body(code, id, State::Empty)?;
                if flag != Some(Flags::Inline) {
                    code.add_char(';');
                }
                Ok(())
            }
        }
    }

    fn index_type(&self, insn: &'a InsnNode) -> Res<&'a ArgType> {
        match &insn.kind {
            InsnKind::Index(IndexValue::Type(t)) => Ok(t),
            _ => Err(CodegenError::new("Missing type index")),
        }
    }

    fn index_field(&self, insn: &'a InsnNode) -> Res<&'a FieldInfo> {
        match &insn.kind {
            InsnKind::Index(IndexValue::Field(f)) => Ok(f),
            _ => Err(CodegenError::new("Missing field index")),
        }
    }

    fn arg_of<'i>(&self, insn: &'i InsnNode, n: usize) -> Res<&'i InsnArg> {
        insn.args
            .get(n)
            .ok_or_else(|| CodegenError::new(format!("Missing argument {n} of {}", insn.ty)))
    }

    fn make_insn_body(&self, code: &mut CodeWriter, id: InsnId, state: State) -> Res {
        let insn = self.insn(id);
        match insn.ty {
            InsnType::ConstStr => {
                let InsnKind::ConstStr(units) = &insn.kind else {
                    return Err(CodegenError::new("Missing string"));
                };
                code.add(&self.env().string_utils.unescape_string(units));
            }
            InsnType::ConstClass => {
                let InsnKind::ConstClass(ty) = &insn.kind else {
                    return Err(CodegenError::new("Missing class type"));
                };
                self.use_type(code, ty)?;
                code.add(".class");
            }
            InsnType::Const => match self.arg_of(insn, 0)? {
                InsnArg::Literal(l) => {
                    let s = self.lit(l)?;
                    code.add(&s);
                }
                _ => return Err(CodegenError::new("Constant without literal")),
            },
            InsnType::Move => self.add_arg(code, self.arg_of(insn, 0)?, false)?,
            InsnType::CheckCast | InsnType::Cast => {
                let wrap = state == State::BodyOnly;
                if wrap {
                    code.add("(");
                }
                code.add("(");
                self.use_type(code, self.index_type(insn)?)?;
                code.add(") ");
                self.add_arg(code, self.arg_of(insn, 0)?, true)?;
                if wrap {
                    code.add(")");
                }
            }
            InsnType::Arith => self.make_arith(code, id, state)?,
            InsnType::Neg => self.one_arg_insn(code, insn, state, '-')?,
            InsnType::Not => {
                let op = if self.arg_type(self.arg_of(insn, 0)?) == ArgType::boolean() {
                    '!'
                } else {
                    '~'
                };
                self.one_arg_insn(code, insn, state, op)?;
            }
            InsnType::Return => {
                if insn.args.is_empty() {
                    code.add("return");
                } else {
                    code.add("return ");
                    self.add_arg(code, &insn.args[0], false)?;
                }
            }
            InsnType::Break => {
                code.add("break");
                if let Some(l) = self.cfg().regions.insn_loop_label.get(&id) {
                    let label = self
                        .mgen
                        .name_gen()
                        .borrow_mut()
                        .loop_label(self.cfg().loop_infos[*l].id);
                    code.add(" ").add(&label);
                }
            }
            InsnType::Continue => {
                code.add("continue");
            }
            InsnType::Throw => {
                code.add("throw ");
                self.add_arg(code, self.arg_of(insn, 0)?, true)?;
            }
            InsnType::CmpL | InsnType::CmpG => {
                let (a, b) = (self.arg_of(insn, 0)?, self.arg_of(insn, 1)?);
                code.add("(");
                self.add_arg(code, a, true)?;
                code.add(" > ");
                self.add_arg(code, b, true)?;
                code.add(" ? 1 : (");
                self.add_arg(code, a, true)?;
                code.add(" == ");
                self.add_arg(code, b, true)?;
                code.add(" ? 0 : -1))");
            }
            InsnType::InstanceOf => {
                let wrap = state == State::BodyOnly;
                if wrap {
                    code.add("(");
                }
                self.add_arg(code, self.arg_of(insn, 0)?, true)?;
                code.add(" instanceof ");
                self.use_type(code, self.index_type(insn)?)?;
                if wrap {
                    code.add(")");
                }
            }
            InsnType::Constructor => self.make_constructor(code, id)?,
            InsnType::Invoke => self.make_invoke(code, id)?,
            InsnType::NewArray => {
                let InsnKind::NewArray(array_type) = &insn.kind else {
                    return Err(CodegenError::new("Missing array type"));
                };
                code.add("new ");
                self.use_type(code, array_type.array_root_element())?;
                for a in &insn.args {
                    code.add("[");
                    self.add_arg(code, a, false)?;
                    code.add("]");
                }
                for _ in insn.args.len()..array_type.array_dimension() as usize {
                    code.add("[]");
                }
            }
            InsnType::ArrayLength => {
                self.add_arg(code, self.arg_of(insn, 0)?, true)?;
                code.add(".length");
            }
            InsnType::FilledNewArray => self.filled_new_array(code, insn)?,
            InsnType::FillArray => {
                if self.fallback {
                    return Err(CodegenError::unsupported("fill-array in fallback mode"));
                }
                self.fill_array(code, insn)?;
            }
            InsnType::Aget => {
                self.add_arg(code, self.arg_of(insn, 0)?, true)?;
                code.add("[");
                self.add_arg(code, self.arg_of(insn, 1)?, false)?;
                code.add("]");
            }
            InsnType::Aput => {
                self.add_arg(code, self.arg_of(insn, 0)?, true)?;
                code.add("[");
                self.add_arg(code, self.arg_of(insn, 1)?, false)?;
                code.add("] = ");
                self.add_arg(code, self.arg_of(insn, 2)?, false)?;
            }
            InsnType::Iget => {
                let f = self.index_field(insn)?;
                self.instance_field(code, f, self.arg_of(insn, 0)?)?;
            }
            InsnType::Iput => {
                let f = self.index_field(insn)?;
                self.instance_field(code, f, self.arg_of(insn, 1)?)?;
                code.add(" = ");
                self.add_arg(code, self.arg_of(insn, 0)?, false)?;
            }
            InsnType::Sget => self.static_field(code, self.index_field(insn)?)?,
            InsnType::Sput => {
                self.static_field(code, self.index_field(insn)?)?;
                code.add(" = ");
                self.add_arg(code, self.arg_of(insn, 0)?, false)?;
            }
            InsnType::StrConcat => {
                let wrap = state == State::BodyOnly;
                if wrap {
                    code.add("(");
                }
                for (i, a) in insn.args.iter().enumerate() {
                    self.add_arg(code, a, true)?;
                    if i + 1 < insn.args.len() {
                        code.add(" + ");
                    }
                }
                if wrap {
                    code.add(")");
                }
            }
            InsnType::MonitorEnter | InsnType::MonitorExit => {
                if self.fallback {
                    return Err(CodegenError::unsupported("monitor in fallback mode"));
                }
            }
            InsnType::Ternary => self.make_ternary(code, id, state)?,
            InsnType::OneArg => self.add_arg_state(code, self.arg_of(insn, 0)?, state)?,
            InsnType::If
            | InsnType::Goto
            | InsnType::MoveException
            | InsnType::Switch
            | InsnType::NewInstance
            | InsnType::Phi
            | InsnType::MoveResult
            | InsnType::FillArrayData
            | InsnType::SwitchData
            | InsnType::MoveMulti
            | InsnType::JavaJsr
            | InsnType::JavaRet => {
                if !self.fallback {
                    return Err(CodegenError::unsupported(format!(
                        "{} instruction can be used only in fallback mode",
                        insn.ty
                    )));
                }
                self.make_fallback_insn(code, insn)?;
            }
            InsnType::RegionArg | InsnType::Nop => {
                return Err(CodegenError::new(format!(
                    "Unknown instruction: {}",
                    insn.ty
                )));
            }
        }
        Ok(())
    }

    /// Instructions that exist only in fallback mode.
    fn make_fallback_insn(&self, code: &mut CodeWriter, insn: &InsnNode) -> Res {
        match insn.ty {
            InsnType::If => {
                let InsnKind::If {
                    op,
                    target,
                    then_block,
                    ..
                } = &insn.kind
                else {
                    return Err(CodegenError::new("Missing if data"));
                };
                code.add("if (");
                self.add_arg(code, self.arg_of(insn, 0)?, true)?;
                code.add(" ").add(op.symbol()).add(" ");
                self.add_arg(code, self.arg_of(insn, 1)?, true)?;
                let label = match then_block {
                    Some(b) => format!("L{b}"),
                    None => label_name(*target),
                };
                code.add(") goto ").add(&label);
            }
            InsnType::Goto => {
                let InsnKind::Goto { target } = &insn.kind else {
                    return Err(CodegenError::new("Missing goto data"));
                };
                code.add("goto ").add(&label_name(*target));
            }
            InsnType::MoveException => {
                code.add("move-exception");
            }
            InsnType::Switch => {
                let InsnKind::Switch { data, def, .. } = &insn.kind else {
                    return Err(CodegenError::new("Missing switch data"));
                };
                code.add("switch(");
                self.add_arg(code, self.arg_of(insn, 0)?, true)?;
                code.add(") {");
                code.inc_indent();
                if let Some(d) = data {
                    for (k, t) in d.keys.iter().zip(&d.targets) {
                        code.start_line_str("case ")
                            .add(&k.to_string())
                            .add(": goto ")
                            .add(&label_name(*t))
                            .add(";");
                    }
                }
                code.start_line_str("default: goto ")
                    .add(&label_name(*def))
                    .add(";");
                code.dec_indent();
                code.start_line_char('}');
            }
            InsnType::NewInstance => {
                let ty = insn
                    .result
                    .as_ref()
                    .map(|r| r.init_type.to_string())
                    .unwrap_or_default();
                code.add("new ").add(&ty);
            }
            InsnType::Phi => {
                code.add(insn.ty.name()).add("(");
                for a in &insn.args {
                    self.add_arg(code, a, true)?;
                    code.add(" ");
                }
                code.add(")");
            }
            InsnType::MoveResult => {
                code.add("move-result");
            }
            InsnType::FillArrayData => {
                code.add("fill-array ").add(&crate::dump::dump_insn(insn));
            }
            InsnType::SwitchData => {
                code.add(&crate::dump::dump_insn(insn));
            }
            InsnType::MoveMulti => {
                let n = insn.args.len();
                let mut i = 0;
                while i + 1 < n {
                    self.add_arg(code, &insn.args[i], true)?;
                    code.add(" = ");
                    self.add_arg(code, &insn.args[i + 1], true)?;
                    code.add("; ");
                    i += 2;
                }
            }
            InsnType::JavaJsr => {
                code.add("jsr -> ");
            }
            InsnType::JavaRet => {
                code.add("ret ");
                self.add_arg(code, self.arg_of(insn, 0)?, true)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Array fill by element assignments (the array is expected to exist already).
    fn fill_array(&self, code: &mut CodeWriter, insn: &InsnNode) -> Res {
        let InsnKind::FillArray {
            data: Some(data), ..
        } = &insn.kind
        else {
            return Err(CodegenError::new("Missing fill array data"));
        };
        code.add("// fill-array-data instruction");
        code.start_line();
        let arr_arg = self.arg_of(insn, 0)?;
        let array_type = self.arg_type(arr_arg);
        let elem_type = match array_type.array_element() {
            Some(e) if array_type.is_type_known() => e.clone(),
            _ => data
                .elem_type
                .select_first()
                .unwrap_or_else(|| data.elem_type.clone()),
        };
        let args = crate::passes::mod_visitor::literal_args(data, &elem_type);
        for (i, a) in args.iter().enumerate() {
            if i != 0 {
                code.add(";");
                code.start_line();
            }
            self.add_arg(code, arr_arg, true)?;
            let InsnArg::Literal(l) = a else {
                return Err(CodegenError::new("Array element is not a literal"));
            };
            code.add("[")
                .add(&i.to_string())
                .add("] = ")
                .add(&self.lit(l)?);
        }
        Ok(())
    }

    fn one_arg_insn(&self, code: &mut CodeWriter, insn: &InsnNode, state: State, op: char) -> Res {
        let wrap = state == State::BodyOnly;
        if wrap {
            code.add("(");
        }
        code.add_char(op);
        self.add_arg(code, self.arg_of(insn, 0)?, true)?;
        if wrap {
            code.add(")");
        }
        Ok(())
    }

    fn filled_new_array(&self, code: &mut CodeWriter, insn: &InsnNode) -> Res {
        let InsnKind::FilledNewArray(elem) = &insn.kind else {
            return Err(CodegenError::new("Missing element type"));
        };
        if !insn.contains_flag(iflag::DECLARE_VAR) {
            code.add("new ");
            self.use_type(code, &ArgType::array(elem.clone()))?;
        }
        code.add("{");
        let c = insn.args.len();
        let mut wrap = 0;
        for (i, a) in insn.args.iter().enumerate() {
            self.add_arg(code, a, false)?;
            if i + 1 < c {
                code.add(", ");
            }
            wrap += 1;
            if wrap == 1000 {
                code.start_line();
                wrap = 0;
            }
        }
        code.add("}");
        Ok(())
    }

    fn make_constructor(&self, code: &mut CodeWriter, id: InsnId) -> Res {
        let insn = self.insn(id);
        let InsnKind::Constructor {
            call_mth,
            call_type,
        } = &insn.kind
        else {
            return Err(CodegenError::new("Missing constructor data"));
        };
        let cls_type = call_mth.decl_class.arg_type();
        if let Some(c) = self.root().resolve_class(cls_type) {
            if self.root().classes[c].is_anonymous() && !self.fallback {
                return self.inline_anonymous_constructor(code, c, id);
            }
        }
        if *call_type == CallType::SelfCall {
            return Err(CodegenError::new(
                "Constructor 'self' invoke must be removed!",
            ));
        }
        let call_node = self.root().resolve_method(call_mth);
        let call_mth_node = call_node.map(|(c, m)| &self.root().classes[c].methods[m]);
        match call_type {
            CallType::Super => {
                code.add("super");
            }
            CallType::This => {
                code.add("this");
            }
            _ => {
                let force_short = self.add_outer_class_instance(code, insn, call_mth_node)?;
                code.add("new ");
                if force_short {
                    self.cls_gen()
                        .add_cls_short_name_forced(code, &call_mth.decl_class);
                } else {
                    self.cls_gen().add_cls_name(code, &call_mth.decl_class);
                }
                if let Some(generic_info) = &insn.generic_info {
                    code.add("<");
                    if generic_info.explicit {
                        for (i, ty) in generic_info.types.iter().enumerate() {
                            if i != 0 {
                                code.add(",");
                            }
                            self.use_type(code, ty)?;
                        }
                    }
                    code.add(">");
                }
            }
        }
        self.generate_method_arguments(code, insn, 0, call_mth_node)
    }

    /// `inlineAnonymousConstructor`: the body of the anonymous class is written at its use.
    fn inline_anonymous_constructor(&self, code: &mut CodeWriter, cls: usize, id: InsnId) -> Res {
        let insn = self.insn(id);
        let root = self.root();
        if self.cls_gen().class_index() == cls {
            return Err(CodegenError::new(format!(
                "Anonymous inner class unlimited recursion detected. Convert class to inner: {}",
                root.classes[cls].info.full_name()
            )));
        }
        let Some(anon) = &root.classes[cls].anonymous else {
            return Err(CodegenError::new("Missing anonymous class attribute"));
        };
        let InsnKind::Constructor { call_mth, .. } = &insn.kind else {
            return Err(CodegenError::new("Missing constructor data"));
        };
        code.add("new ");
        self.use_type(code, &anon.base_type)?;
        let call_node = root.resolve_method(call_mth);
        let call_mth_node = call_node.map(|(c, m)| &root.classes[c].methods[m]);
        self.generate_method_arguments(code, insn, 0, call_mth_node)?;
        code.add(" ");

        let cls_gen = ClassGen::with_parent(self.env(), cls, self.cls_gen().parent_gen());
        cls_gen.set_outer_name_gen(self.mgen.name_gen().borrow().clone());
        cls_gen.set_field_replace_names(build_field_replace_names(
            root,
            cls,
            insn,
            &self.mgen.name_gen().borrow(),
            self.cfg(),
        ));
        cls_gen.set_inlined(true);
        cls_gen.add_class_body(code, true)
    }

    fn add_outer_class_instance(
        &self,
        code: &mut CodeWriter,
        insn: &InsnNode,
        call_mth: Option<&MethodNode>,
    ) -> Res<bool> {
        let Some(call_mth) = call_mth.filter(|m| m.skip_first_arg) else {
            return Ok(false);
        };
        let root = self.root();
        let Some(ctr_cls) = root.class_index(&call_mth.info.decl_class.raw_name()) else {
            return Ok(false);
        };
        let Some(inst_arg) = insn.args.first() else {
            return Ok(false);
        };
        if !root.classes[ctr_cls].is_inner() {
            return Ok(false);
        }
        if is_this(inst_arg) {
            return Ok(false);
        }
        // instance arg should be of an outer class type
        let Some(outer) = root.parent_index(ctr_cls) else {
            return Ok(false);
        };
        if &self.arg_type(inst_arg) != root.classes[outer].info.arg_type() {
            return Ok(false);
        }
        self.add_arg_dot(code, inst_arg)?;
        // can't use another dot, force short name of class
        Ok(true)
    }

    fn make_invoke(&self, code: &mut CodeWriter, id: InsnId) -> Res {
        let insn = self.insn(id);
        let InsnKind::Invoke { invoke_type, mth } = &insn.kind else {
            return Err(CodegenError::new("Missing invoke data"));
        };
        if matches!(invoke_type, InvokeType::Custom | InvokeType::CustomRaw) {
            return Err(CodegenError::unsupported("invoke-custom"));
        }
        let call_node = self.root().resolve_method(mth);
        let call_mth_node = call_node.map(|(c, m)| &self.root().classes[c].methods[m]);
        if is_polymorphic_call(*invoke_type, mth) {
            // add missing cast
            code.add("(");
            self.use_type(code, &mth.return_type)?;
            code.add(") ");
        }
        let mut k = 0;
        match invoke_type {
            InvokeType::Direct
            | InvokeType::Virtual
            | InvokeType::Interface
            | InvokeType::Polymorphic => {
                let arg = self.arg_of(insn, 0)?;
                if self.need_invoke_arg(arg) {
                    self.add_arg_dot(code, arg)?;
                }
                k += 1;
            }
            InvokeType::Super => {
                self.call_super(code, mth)?;
                // use 'super' instead 'this' in 0 arg
                k += 1;
                code.add(".");
            }
            InvokeType::Static => {
                let insn_cls = self.cls_gen().class_info();
                if insn_cls != &mth.decl_class {
                    self.use_class_info(code, &mth.decl_class)?;
                    code.add(".");
                }
            }
            InvokeType::Custom | InvokeType::CustomRaw => {}
        }
        code.add(&self.root().method_name(mth));
        self.generate_method_arguments(code, insn, k, call_mth_node)
    }

    /// FIXME in the reference: add 'this' for equals methods in scope.
    fn need_invoke_arg(&self, arg: &InsnArg) -> bool {
        if self.is_any_this(arg) {
            if is_this(arg) {
                return false;
            }
            if let Some(c) = self.root().resolve_class(&self.arg_type(arg)) {
                if self.root().classes[c].dont_generate {
                    return false;
                }
            }
        }
        true
    }

    /// `isAnyThis`: `this` of this or another class (`this` of outer classes occur in anonymous
    /// classes).
    fn is_any_this(&self, arg: &InsnArg) -> bool {
        if is_this(arg) {
            return true;
        }
        if let InsnArg::Wrapped(w) = arg {
            let wrapped = self.insn(w.insn);
            if wrapped.ty == InsnType::Iget {
                return wrapped.args.first().is_some_and(|a| self.is_any_this(a));
            }
        }
        false
    }

    fn call_super(&self, code: &mut CodeWriter, call_mth: &MethodInfo) -> Res {
        let Some(super_call_cls) = self.class_for_super_call(call_mth) else {
            // unknown class, add comment to keep that info
            code.add("super/*")
                .add(call_mth.decl_class.full_name())
                .add("*/");
            return Ok(());
        };
        if &super_call_cls == self.cls_gen().class_info() {
            code.add("super");
            return Ok(());
        }
        // use custom class
        self.use_class_info(code, &super_call_cls)?;
        code.add(".super");
        Ok(())
    }

    /// Search call class in super types of this and all parent classes (needed for inlined
    /// synthetic calls).
    fn class_for_super_call(&self, call_mth: &MethodInfo) -> Option<ClassInfo> {
        let decl_type = call_mth.decl_class.arg_type();
        let root = self.root();
        let mut cur = self.cls_gen().class_index();
        loop {
            let info = &root.classes[cur].info;
            if is_instance_of(root, info.arg_type(), decl_type) {
                return Some(info.clone());
            }
            match root.parent_index(cur) {
                Some(p) if p != cur => cur = p,
                // no parent, class not found
                _ => return None,
            }
        }
    }

    pub(super) fn generate_method_arguments(
        &self,
        code: &mut CodeWriter,
        insn: &InsnNode,
        start_arg_num: usize,
        mth_node: Option<&MethodNode>,
    ) -> Res {
        let mut k = start_arg_num;
        if mth_node.is_some_and(|m| m.skip_first_arg) {
            k += 1;
        }
        let args_count = insn.args.len();
        code.add("(");
        let skip = mth_node.and_then(|m| m.skip_args.as_ref());
        let mut first_arg = true;
        for i in k..args_count {
            let arg = &insn.args[i];
            if skip.is_some_and(|s| s.get(i - start_arg_num).copied().unwrap_or(false)) {
                continue;
            }
            if first_arg {
                first_arg = false;
            } else {
                code.add(", ");
            }
            if i == args_count - 1 && self.process_var_arg(code, insn, arg)? {
                continue;
            }
            self.add_arg(code, arg, false)?;
        }
        code.add(")");
        Ok(())
    }

    /// Expand varargs from filled array.
    fn process_var_arg(
        &self,
        code: &mut CodeWriter,
        invoke: &InsnNode,
        last: &InsnArg,
    ) -> Res<bool> {
        if !invoke.contains_flag(iflag::VARARG_CALL) {
            return Ok(false);
        }
        let InsnArg::Wrapped(w) = last else {
            return Ok(false);
        };
        if !self.arg_type(last).is_array() {
            return Ok(false);
        }
        let insn = self.insn(w.insn);
        if insn.ty != InsnType::FilledNewArray {
            return Ok(false);
        }
        let count = insn.args.len();
        if count == 1 && self.is_array_like_var_arg(&insn.args[0]) {
            // single element will be used as varargs array itself, keep array creation
            return Ok(false);
        }
        for (i, elem) in insn.args.iter().enumerate() {
            self.add_arg(code, elem, false)?;
            if i + 1 < count {
                code.add(", ");
            }
        }
        Ok(true)
    }

    fn is_array_like_var_arg(&self, arg: &InsnArg) -> bool {
        let ty = self.arg_type(arg);
        if self.is_zero_const(arg) {
            return ty.is_object();
        }
        ty.array_element().is_some_and(|e| !e.is_primitive())
    }

    fn is_zero_const(&self, arg: &InsnArg) -> bool {
        match arg {
            InsnArg::Literal(l) => l.literal == 0,
            InsnArg::Wrapped(w) => {
                let i = self.insn(w.insn);
                i.ty == InsnType::Const
                    && matches!(i.args.first(), Some(InsnArg::Literal(l)) if l.literal == 0)
            }
            _ => false,
        }
    }

    fn make_ternary(&self, code: &mut CodeWriter, id: InsnId, state: State) -> Res {
        let insn = self.insn(id);
        let cond = match &insn.kind {
            InsnKind::Ternary { cond } => IfCondition {
                mode: CondMode::Compare,
                args: Vec::new(),
                compare: Some(*cond),
                dont_wrap: false,
            },
            InsnKind::TernaryCond(c) => (**c).clone(),
            _ => return Err(CodegenError::new("Missing ternary condition")),
        };
        let wrap = state == State::BodyOnly;
        if wrap {
            code.add("(");
        }
        let first = self.arg_of(insn, 0)?;
        let second = self.arg_of(insn, 1)?;
        if is_true(first) && is_false(second) {
            self.cond_add(code, &cond)?;
        } else {
            self.cond_wrap(code, &cond)?;
            code.add(" ? ");
            self.add_arg(code, first, false)?;
            code.add(" : ");
            self.add_arg(code, second, false)?;
        }
        if wrap {
            code.add(")");
        }
        Ok(())
    }

    fn make_arith(&self, code: &mut CodeWriter, id: InsnId, state: State) -> Res {
        let insn = self.insn(id);
        let InsnKind::Arith(op) = insn.kind else {
            return Err(CodegenError::new("Missing arithmetic operation"));
        };
        if insn.contains_flag(iflag::ARITH_ONEARG) {
            return self.make_arith_one_arg(code, insn, op);
        }
        // wrap insn in brackets for save correct operation order
        let wrap = state == State::BodyOnly && !insn.contains_flag(iflag::DONT_WRAP);
        if wrap {
            code.add("(");
        }
        self.add_arg(code, self.arg_of(insn, 0)?, true)?;
        code.add(" ");
        code.add(op.symbol());
        code.add(" ");
        self.add_arg(code, self.arg_of(insn, 1)?, true)?;
        if wrap {
            code.add(")");
        }
        Ok(())
    }

    fn make_arith_one_arg(&self, code: &mut CodeWriter, insn: &InsnNode, op: ArithOp) -> Res {
        let res_arg = self.arg_of(insn, 0)?;
        let arg = self.arg_of(insn, 1)?;
        // "++" or "--"
        if let InsnArg::Literal(l) = arg {
            if matches!(op, ArithOp::Add | ArithOp::Sub) && l.literal == 1 && is_integer(&l.ty) {
                self.add_arg(code, res_arg, false)?;
                code.add(op.symbol()).add(op.symbol());
                return Ok(());
            }
        }
        // +=, -=, ...
        self.add_arg(code, res_arg, false)?;
        code.add(" ").add(op.symbol()).add("= ");
        self.add_arg(code, arg, false)
    }
}

/// `MethodGen.getLabelName(int)`
pub fn label_name(offset: i32) -> String {
    if offset < 0 {
        format!("LB_{:x}", -offset)
    } else {
        format!("L{offset:x}")
    }
}

/// `makeStaticFieldAccess`
pub fn make_static_field_access(
    code: &mut CodeWriter,
    field: &FieldInfo,
    cls_gen: &ClassGen<'_>,
) -> Res {
    let decl_class = &field.decl_class;
    let field_from_this_class = cls_gen.class_info() == decl_class;
    if !field_from_this_class || !cls_gen.is_body_gen_started() {
        // Android specific resources class handler
        if !handle_app_res_field(code, cls_gen, decl_class)? {
            cls_gen.use_class_info(code, decl_class)?;
        }
        code.add(".");
    }
    code.add(&cls_gen.env().root.field_name(field));
    Ok(())
}

/// `AndroidResourcesUtils.handleAppResField`
fn handle_app_res_field(
    code: &mut CodeWriter,
    cls_gen: &ClassGen<'_>,
    decl_class: &ClassInfo,
) -> Res<bool> {
    match decl_class.parent_class() {
        Some(parent) if parent.short_name() == "R" => {
            cls_gen.use_class_info(code, parent)?;
            code.add(".");
            code.add(&cls_gen.env().short_name(decl_class));
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// `ArgType.isInstanceOf`
fn is_instance_of(root: &Root, ty: &ArgType, of: &ArgType) -> bool {
    if ty == of {
        return true;
    }
    if !ty.is_object() || !of.is_object() {
        return false;
    }
    match (ty.object_name(), of.object_name()) {
        (Some(a), Some(b)) => root.clsp.is_implements(&a, &b),
        _ => false,
    }
}

/// `InvokeNode.isPolymorphicCall`
fn is_polymorphic_call(invoke_type: InvokeType, mth: &MethodInfo) -> bool {
    if invoke_type == InvokeType::Polymorphic {
        return true;
    }
    // java bytecode uses virtual call with modified method info
    invoke_type == InvokeType::Virtual
        && mth.decl_class.full_name() == "java.lang.invoke.MethodHandle"
        && (mth.name == "invoke" || mth.name == "invokeExact")
}

/// `InsnArg.isThis`
fn is_this(arg: &InsnArg) -> bool {
    matches!(arg, InsnArg::Register(r) if r.is_this)
}

/// `InsnArg.isTrue`
fn is_true(arg: &InsnArg) -> bool {
    matches!(arg, InsnArg::Literal(l) if l.literal == 1 && l.ty == ArgType::boolean())
}

/// `InsnArg.isFalse`
fn is_false(arg: &InsnArg) -> bool {
    matches!(arg, InsnArg::Literal(l) if l.literal == 0 && l.ty == ArgType::boolean())
}

/// `LiteralArg.isInteger`
fn is_integer(ty: &ArgType) -> bool {
    matches!(
        ty.primitive_type(),
        Some(
            PrimitiveType::Int
                | PrimitiveType::Byte
                | PrimitiveType::Char
                | PrimitiveType::Short
                | PrimitiveType::Long
        )
    ) && ty.is_primitive()
}
