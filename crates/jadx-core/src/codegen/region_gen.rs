//! `RegionGen`: code for the region tree of a method (conditions, loops, switches, try/catch,
//! synchronized blocks and plain block sequences).

use super::code_writer::CodeWriter;
use super::insn_gen::{Flags, InsnGen};
use super::method_gen::MethodGen;
use super::type_gen::literal_to_string;
use super::CodegenError;
use crate::model::cfg::{bflag, MethodCfg};
use crate::model::info::FieldInfo;
use crate::model::insn::{iflag, HandlerId, InsnArg, InsnKind, InsnType, NamedArg};
use crate::model::region::{CaseKey, Container, LoopType, RegionId, RegionKind};
use crate::model::types::ArgType;
use crate::passes::block_utils::get_last_insn;
use crate::passes::regions::utils as region_utils;

type Res<T = ()> = Result<T, CodegenError>;

pub struct RegionGen<'m, 'a> {
    ig: InsnGen<'m, 'a>,
}

impl<'m, 'a> RegionGen<'m, 'a> {
    pub fn new(mgen: &'m MethodGen<'a>) -> Self {
        RegionGen {
            ig: InsnGen::new(mgen, false),
        }
    }

    pub fn insn_gen(&self) -> &InsnGen<'m, 'a> {
        &self.ig
    }

    fn cfg(&self) -> &'a MethodCfg {
        self.ig.cfg()
    }

    /// `makeRegion`
    pub fn make_region(&self, code: &mut CodeWriter, cont: Container) -> Res {
        self.declare_vars(code, cont)?;
        self.generate(code, cont)
    }

    /// `IContainer.generate`
    fn generate(&self, code: &mut CodeWriter, cont: Container) -> Res {
        match cont {
            Container::Block(b) => self.make_simple_block(code, cont, self.cfg().block(b).flags),
            Container::Insns(i) => {
                self.make_simple_block(code, cont, self.cfg().regions.insn_conts[i].flags)
            }
            Container::Region(r) => self.generate_region(code, r),
        }
    }

    fn generate_region(&self, code: &mut CodeWriter, r: RegionId) -> Res {
        let cfg = self.cfg();
        match &cfg.regions.arena[r].kind {
            RegionKind::Plain { blocks } => {
                for c in blocks {
                    self.make_region(code, *c)?;
                }
                Ok(())
            }
            RegionKind::If { .. } => self.make_if(code, r, true),
            RegionKind::Loop { .. } => self.make_loop(code, r),
            RegionKind::Switch { .. } => self.make_switch(code, r),
            RegionKind::Sync { .. } => self.make_synchronized(code, r),
            RegionKind::TryCatch { .. } => self.make_try_catch(code, r),
        }
    }

    fn declare_vars(&self, code: &mut CodeWriter, cont: Container) -> Res {
        let Some(vars) = self.cfg().regions.declare_vars.get(&cont) else {
            return Ok(());
        };
        for &v in vars {
            code.start_line();
            self.ig.declare_code_var(code, v)?;
            code.add_char(';');
        }
        Ok(())
    }

    fn make_region_indent(&self, code: &mut CodeWriter, region: Container) -> Res {
        code.inc_indent();
        let r = self.make_region(code, region);
        code.dec_indent();
        r
    }

    /// `makeSimpleBlock`
    fn make_simple_block(&self, code: &mut CodeWriter, cont: Container, flags: u64) -> Res {
        if flags & bflag::DONT_GENERATE != 0 {
            return Ok(());
        }
        let cfg = self.cfg();
        for insn in region_utils::container_insns(cfg, cont) {
            let node = cfg.insn(insn);
            // instructions that have no source code (removed before the code generation)
            if matches!(
                node.ty,
                InsnType::Nop
                    | InsnType::MonitorEnter
                    | InsnType::MonitorExit
                    | InsnType::MoveException
            ) {
                continue;
            }
            if !node.contains_flag(iflag::DONT_GENERATE) {
                self.ig.make_insn(insn, code, None)?;
            }
        }
        Ok(())
    }

    /// `makeIf`
    fn make_if(&self, code: &mut CodeWriter, r: RegionId, new_line: bool) -> Res {
        let cfg = self.cfg();
        let RegionKind::If {
            cond,
            then_r,
            else_r,
            ..
        } = &cfg.regions.arena[r].kind
        else {
            return Err(CodegenError::new("Not an if region"));
        };
        if new_line {
            code.start_line();
        }
        code.add("if (");
        let cond = cond
            .as_ref()
            .ok_or_else(|| CodegenError::new("If region without condition"))?;
        self.ig.cond_add(code, cond)?;
        code.add(") {");
        if let Some(t) = then_r {
            self.make_region_indent(code, *t)?;
        }
        code.start_line_char('}');

        if region_utils::not_empty(cfg, *else_r) {
            let els = else_r.expect("else region");
            code.add(" else ");
            if self.connect_else_if(code, els)? {
                return Ok(());
            }
            code.add_char('{');
            self.make_region_indent(code, els)?;
            code.start_line_char('}');
        }
        Ok(())
    }

    /// Connect if-else-if block.
    fn connect_else_if(&self, code: &mut CodeWriter, els: Container) -> Res<bool> {
        let cfg = self.cfg();
        if region_utils::cflag(cfg, els, bflag::ELSE_IF_CHAIN) {
            if let Some(Container::Region(inner)) = region_utils::get_single_sub_block(cfg, els) {
                if matches!(cfg.regions.arena[inner].kind, RegionKind::If { .. }) {
                    self.declare_vars(code, Container::Region(inner))?;
                    self.make_if(code, inner, false)?;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// `makeLoop`
    fn make_loop(&self, code: &mut CodeWriter, r: RegionId) -> Res {
        let cfg = self.cfg();
        let RegionKind::Loop {
            info,
            cond,
            body,
            ty,
            condition_at_end,
            ..
        } = &cfg.regions.arena[r].kind
        else {
            return Err(CodegenError::new("Not a loop region"));
        };
        code.start_line();
        let start = cfg.loop_infos[*info].start;
        if let Some(l) = cfg.regions.block_loop_label.get(&start) {
            let label = self
                .ig
                .mgen()
                .name_gen()
                .borrow_mut()
                .loop_label(cfg.loop_infos[*l].id);
            code.add(&label).add(": ");
        }
        let body = body.map(Container::Region);
        let Some(cond) = cond else {
            // infinite loop
            code.add("while (true) {");
            if let Some(b) = body {
                self.make_region_indent(code, b)?;
            }
            code.start_line_char('}');
            return Ok(());
        };
        if let Some(ty) = ty {
            match ty {
                LoopType::For { init, incr } => {
                    code.add("for (");
                    self.ig.make_insn(*init, code, Some(Flags::Inline))?;
                    code.add("; ");
                    self.ig.cond_add(code, cond)?;
                    code.add("; ");
                    self.ig.make_insn(*incr, code, Some(Flags::Inline))?;
                    code.add(") {");
                }
                LoopType::ForEach {
                    var_arg_insn,
                    iterable_insn,
                } => {
                    code.add("for (");
                    let var = cfg
                        .insn(*var_arg_insn)
                        .result
                        .as_ref()
                        .ok_or_else(|| CodegenError::new("For each loop without variable"))?;
                    self.ig.declare_var(code, var)?;
                    code.add(" : ");
                    let iterable = cfg
                        .insn(*iterable_insn)
                        .args
                        .first()
                        .ok_or_else(|| CodegenError::new("For each loop without iterable"))?;
                    self.ig.add_arg(code, iterable, false)?;
                    code.add(") {");
                }
            }
            if let Some(b) = body {
                self.make_region_indent(code, b)?;
            }
            code.start_line_char('}');
            return Ok(());
        }
        if *condition_at_end {
            code.add("do {");
            if let Some(b) = body {
                self.make_region_indent(code, b)?;
            }
            code.start_line();
            code.add("} while (");
            self.ig.cond_add(code, cond)?;
            code.add(");");
        } else {
            code.add("while (");
            self.ig.cond_add(code, cond)?;
            code.add(") {");
            if let Some(b) = body {
                self.make_region_indent(code, b)?;
            }
            code.start_line_char('}');
        }
        Ok(())
    }

    /// `makeSynchronizedRegion`
    fn make_synchronized(&self, code: &mut CodeWriter, r: RegionId) -> Res {
        let cfg = self.cfg();
        let RegionKind::Sync {
            enter_insn, inner, ..
        } = &cfg.regions.arena[r].kind
        else {
            return Err(CodegenError::new("Not a synchronized region"));
        };
        code.start_line_str("synchronized (");
        let arg = cfg
            .insn(*enter_insn)
            .args
            .first()
            .ok_or_else(|| CodegenError::new("Monitor enter without argument"))?;
        self.ig.add_arg(code, arg, true)?;
        code.add(") {");
        self.make_region_indent(code, Container::Region(*inner))?;
        code.start_line_char('}');
        Ok(())
    }

    /// `makeSwitch`
    fn make_switch(&self, code: &mut CodeWriter, r: RegionId) -> Res {
        let cfg = self.cfg();
        let RegionKind::Switch { header, cases } = &cfg.regions.arena[r].kind else {
            return Err(CodegenError::new("Not a switch region"));
        };
        let insn_id = get_last_insn(cfg, *header)
            .ok_or_else(|| CodegenError::new("Switch insn not found in header"))?;
        let insn = cfg.insn(insn_id);
        let arg = insn
            .args
            .first()
            .ok_or_else(|| CodegenError::new("Switch without argument"))?;
        code.start_line_str("switch (");
        self.ig.add_arg(code, arg, false)?;
        code.add(") {");
        code.inc_indent();
        for case in cases {
            for k in &case.keys {
                if *k == CaseKey::Default {
                    code.start_line_str("default:");
                } else {
                    code.start_line_str("case ");
                    self.add_case_key(code, insn_id, arg, k)?;
                    code.add_char(':');
                }
            }
            self.make_region_indent(code, case.container)?;
        }
        code.dec_indent();
        code.start_line_char('}');
        Ok(())
    }

    fn add_case_key(
        &self,
        code: &mut CodeWriter,
        switch_insn: crate::model::insn::InsnId,
        arg: &InsnArg,
        k: &CaseKey,
    ) -> Res {
        match k {
            CaseKey::Int(i) => {
                if let Some(f) = self.key_field(switch_insn, *i) {
                    return self.use_field(code, &f);
                }
                let s = literal_to_string(
                    i64::from(*i),
                    &self.ig.arg_type(arg),
                    &self.ig.env().string_utils,
                    self.ig.is_fallback(),
                    false,
                )?;
                code.add(&s);
                Ok(())
            }
            CaseKey::Str(s) => {
                code.add("\"").add(s).add("\"");
                Ok(())
            }
            CaseKey::Default => Err(CodegenError::new("Unexpected key in switch")),
        }
    }

    /// Constant field that replaced the key of the switch case.
    fn key_field(&self, switch_insn: crate::model::insn::InsnId, key: i32) -> Option<FieldInfo> {
        let InsnKind::Switch { data: Some(d), .. } = &self.cfg().insn(switch_insn).kind else {
            return None;
        };
        let pos = d.keys.iter().position(|k| *k == key)?;
        d.key_fields.get(pos).cloned().flatten()
    }

    /// `useField`
    fn use_field(&self, code: &mut CodeWriter, fld: &FieldInfo) -> Res {
        let env = self.ig.env();
        let root = env.root;
        let node = root.resolve_field(fld);
        let is_enum = match node {
            Some(_) => root
                .class_index(&fld.decl_class.raw_name())
                .is_some_and(|c| root.classes[c].is_enum()),
            None => root
                .clsp
                .cls_details(&fld.decl_class.raw_name())
                .is_some_and(|d| d.access_flags & crate::access::ACC_ENUM != 0),
        };
        if is_enum {
            code.add(&root.field_name(fld));
            return Ok(());
        }
        super::insn_gen::make_static_field_access(code, fld, self.ig.cls_gen())?;
        if let Some(n) = node {
            if let Some(v) = &n.const_value {
                if self.ig.mgen().class_gen().comments_level_info() {
                    code.add(" /* ")
                        .add(&super::annotation_gen::const_value_to_string(v))
                        .add(" */");
                }
            }
        }
        Ok(())
    }

    /// `makeTryCatch`
    fn make_try_catch(&self, code: &mut CodeWriter, r: RegionId) -> Res {
        let cfg = self.cfg();
        let RegionKind::TryCatch {
            try_r,
            catches,
            finally_r,
            ..
        } = &cfg.regions.arena[r].kind
        else {
            return Err(CodegenError::new("Not a try/catch region"));
        };
        code.start_line_str("try {");
        self.make_region_indent(code, *try_r)?;
        let mut all_handler: Option<HandlerId> = None;
        for (h, _) in catches {
            if cfg.handlers[*h].is_catch_all() {
                all_handler = Some(*h);
            } else {
                self.make_catch_block(code, *h)?;
            }
        }
        if let Some(h) = all_handler {
            self.make_catch_block(code, h)?;
        }
        if let Some(f) = finally_r {
            code.start_line_str("} finally {");
            self.make_region_indent(code, *f)?;
        }
        code.start_line_char('}');
        Ok(())
    }

    fn make_catch_block(&self, code: &mut CodeWriter, h: HandlerId) -> Res {
        let cfg = self.cfg();
        let Some(region) = cfg.regions.handler_regions.get(&h).copied() else {
            return Ok(());
        };
        let handler = &cfg.handlers[h];
        code.start_line_str("} catch (");
        if handler.is_catch_all() {
            self.ig.use_type(code, &ArgType::throwable())?;
        } else {
            for (i, ci) in handler.catch_types.iter().enumerate() {
                if i != 0 {
                    code.add(" | ");
                }
                self.ig.use_class_info(code, ci)?;
            }
        }
        code.add_char(' ');
        let name_gen = self.ig.mgen().name_gen();
        if let Some((name, _)) = &handler.named_arg {
            let n = name_gen.borrow_mut().assign_named_arg(name);
            code.add(&n);
        } else if let Some(cv) = self.handler_code_var(h) {
            let n = name_gen.borrow_mut().assign_arg(cfg, cv);
            code.add(&n);
        } else {
            // handler arguments not used
            let n = name_gen.borrow_mut().assign_named_arg("unused");
            code.add(&n);
        }
        code.add(") {");
        self.make_region_indent(code, region)
    }

    /// Variable of the `move-exception` result of the handler.
    fn handler_code_var(&self, h: HandlerId) -> Option<crate::model::ssa::CodeVarId> {
        let cfg = self.cfg();
        cfg.handlers[h].arg_reg?;
        for blk in &cfg.handlers[h].blocks {
            for &i in &cfg.block(*blk).insns {
                let insn = cfg.insn(i);
                if insn.exc_handler == Some(h) && insn.ty == InsnType::MoveException {
                    return insn
                        .result
                        .as_ref()
                        .and_then(|r| r.svar)
                        .and_then(|v| cfg.svars[v].code_var);
                }
            }
        }
        for (i, insn) in cfg.insns.iter().enumerate() {
            let _ = i;
            if insn.exc_handler == Some(h) && insn.ty == InsnType::MoveException {
                return insn
                    .result
                    .as_ref()
                    .and_then(|r| r.svar)
                    .and_then(|v| cfg.svars[v].code_var);
            }
        }
        None
    }
}

#[allow(dead_code)]
fn unused(_: NamedArg) {}
