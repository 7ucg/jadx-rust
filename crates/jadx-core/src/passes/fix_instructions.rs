//! `FixInstructionsVisitor`: verify and fix instructions (string concatenation arguments).

use crate::model::cfg::MethodCfg;
use crate::model::insn::{InsnArg, InsnId, InsnKind, InsnNode, InsnType};
use crate::model::region::Container;
use crate::model::ssa::ArgRef;
use crate::model::types::ArgType;
use crate::passes::regions::maker::Res;
use crate::passes::regions::traversal::{traverse_method, RegionVisitor};
use crate::passes::regions::utils::container_insns;

pub fn fix_instructions(cfg: &mut MethodCfg) -> Result<(), String> {
    traverse_method(cfg, &mut Fix)
}

struct Fix;

impl RegionVisitor for Fix {
    fn process_block(&mut self, cfg: &mut MethodCfg, block: Container) -> Res<()> {
        for insn in container_insns(cfg, block) {
            let mut all = Vec::new();
            visit_insns(cfg, insn, &mut all);
            for i in all {
                fix_insn(cfg, i)?;
            }
        }
        Ok(())
    }
}

/// `InsnNode.visitInsns`: the instruction, then the wrapped instructions of its arguments (and
/// the condition of a ternary).
pub fn visit_insns(cfg: &MethodCfg, insn: InsnId, out: &mut Vec<InsnId>) {
    out.push(insn);
    for a in &cfg.insn(insn).args {
        if let InsnArg::Wrapped(w) = a {
            visit_insns(cfg, w.insn, out);
        }
    }
    match &cfg.insn(insn).kind {
        InsnKind::Ternary { cond } => visit_insns(cfg, *cond, out),
        InsnKind::TernaryCond(c) => {
            for i in c.collect_insns() {
                visit_insns(cfg, i, out);
            }
        }
        _ => {}
    }
}

fn fix_insn(cfg: &mut MethodCfg, insn: InsnId) -> Result<(), String> {
    if cfg.insn(insn).ty == InsnType::StrConcat && force_string_concat(cfg, insn) {
        // insert empty string arg to force '+' to use string concatenation
        let id = cfg.add_insn(InsnNode::new(
            InsnType::ConstStr,
            InsnKind::ConstStr(Vec::new()),
        ));
        let arg = cfg.wrap_arg(id)?;
        insert_arg_front(cfg, insn, arg);
    }
    Ok(())
}

fn force_string_concat(cfg: &MethodCfg, insn: InsnId) -> bool {
    let args_count = cfg.insn(insn).args.len();
    if args_count == 0 || cfg.arg_type(ArgRef::Arg(insn, 0)) == ArgType::string() {
        return false;
    }
    // a single non-string arg (`c + ""`) also needs the empty string to stay a string
    args_count == 1 || cfg.arg_type(ArgRef::Arg(insn, 1)) != ArgType::string()
}

/// `insn.getArgList().add(0, arg)`: the register arguments move one place up.
fn insert_arg_front(cfg: &mut MethodCfg, insn: InsnId, arg: InsnArg) {
    let count = cfg.insn(insn).args.len();
    for k in (0..count).rev() {
        if let InsnArg::Register(r) = &cfg.insn(insn).args[k] {
            if let Some(v) = r.svar {
                for u in cfg.svars[v].uses.iter_mut() {
                    if *u == ArgRef::Arg(insn, k) {
                        *u = ArgRef::Arg(insn, k + 1);
                    }
                }
            }
        }
    }
    cfg.insns[insn].args.insert(0, arg);
}
