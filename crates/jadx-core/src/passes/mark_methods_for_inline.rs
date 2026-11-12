//! `MarkMethodsForInline`: analyze the candidates for inlining (synthetic static methods and
//! constructors of anonymous classes) and store the instruction that replaces their calls.

use crate::model::cfg::MethodCfg;
use crate::model::insn::{iflag, IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType};
use crate::model::node::{InlineAttr, MethodId, Root, ACC_PUBLIC};
use crate::passes::insn_utils::same_reg_and_svar;

/// Result of the analysis of a method: the attribute and the methods that need public visibility
/// to be called from the inlined code.
pub struct Marked {
    pub attr: InlineAttr,
    pub make_public: Vec<MethodId>,
}

/// `MarkMethodsForInline.process`: `None` if the method can't be analyzed (code not loaded).
/// The caller has to check that the method has no attribute yet.
pub fn process(
    root: &Root,
    id: MethodId,
    cfg: Option<&MethodCfg>,
    respect_acc_modifiers: bool,
) -> Option<Marked> {
    if root.classes[id.0].methods[id.1].inline_candidate {
        let cfg = cfg?;
        if let Some(m) = inline_mth(root, cfg, respect_acc_modifiers) {
            return Some(m);
        }
    }
    Some(Marked {
        attr: InlineAttr::not_needed(),
        make_public: Vec::new(),
    })
}

/// `BlockUtils.collectInsnsWithLimit`
fn collect_insns_with_limit(cfg: &MethodCfg, limit: usize) -> Vec<InsnId> {
    let mut insns = Vec::new();
    for &b in &cfg.blocks {
        let list = &cfg.block(b).insns;
        if list.is_empty() {
            continue;
        }
        if insns.len() + list.len() > limit {
            return Vec::new();
        }
        insns.extend(list.iter().copied());
    }
    insns
}

fn inline_mth(root: &Root, cfg: &MethodCfg, respect: bool) -> Option<Marked> {
    let insns = collect_insns_with_limit(cfg, 2);
    match insns.len() {
        0 => None,
        1 => {
            let insn = cfg.insn(insns[0]);
            if insn.ty == InsnType::Return && insn.args.len() == 1 {
                // synthetic field getter
                let InsnArg::Wrapped(w) = &insn.args[0] else {
                    return None;
                };
                return add_inline_attr(root, cfg, w.insn, true, respect);
            }
            // method invoke
            add_inline_attr(root, cfg, insns[0], false, respect)
        }
        2 if cfg.insn(insns[1]).ty == InsnType::Return => {
            let ret = cfg.insn(insns[1]);
            if ret.args.is_empty() || is_synthetic_access_pattern(cfg, insns[0], insns[1]) {
                return add_inline_attr(root, cfg, insns[0], false, respect);
            }
            None
        }
        _ => None,
    }
}

/// `InsnArg.isSameVar(RegisterArg)`
fn is_same_var(arg: Option<&InsnArg>, reg: Option<&crate::model::insn::RegisterArg>) -> bool {
    match (arg, reg) {
        (Some(InsnArg::Register(a)), Some(b)) => same_reg_and_svar(a, b),
        _ => false,
    }
}

fn is_synthetic_access_pattern(cfg: &MethodCfg, first: InsnId, ret: InsnId) -> bool {
    let regs = &cfg.arg_regs;
    let first = cfg.insn(first);
    let ret = cfg.insn(ret);
    match first.ty {
        InsnType::Iget => {
            regs.len() == 1
                && is_same_var(ret.args.first(), first.result.as_ref())
                && is_same_var(first.args.first(), regs.first())
        }
        InsnType::Sget => regs.is_empty() && is_same_var(ret.args.first(), first.result.as_ref()),
        InsnType::Iput => {
            regs.len() == 2
                && is_same_var(ret.args.first(), regs.get(1))
                && is_same_var(first.args.first(), regs.get(1))
                && is_same_var(first.args.get(1), regs.first())
        }
        InsnType::Sput => {
            regs.len() == 1
                && is_same_var(ret.args.first(), regs.first())
                && is_same_var(first.args.first(), regs.first())
        }
        InsnType::Invoke => {
            if !is_same_var(ret.args.first(), first.result.as_ref()) {
                return false;
            }
            regs.len() == first.args.len()
                && regs
                    .iter()
                    .zip(first.args.iter())
                    .all(|(m, a)| is_same_var(Some(a), Some(m)))
        }
        _ => false,
    }
}

fn add_inline_attr(
    root: &Root,
    cfg: &MethodCfg,
    insn: InsnId,
    is_copy: bool,
    respect: bool,
) -> Option<Marked> {
    let mut make_public = Vec::new();
    if !fix_visibility_of_inline_code(root, cfg, insn, respect, &mut make_public) {
        return None;
    }
    let mut insns = Vec::new();
    let new_root = extract_tree(cfg, insn, &mut insns);
    if is_copy {
        insns[new_root].flags &= !iflag::WRAPPED;
    }
    let mut args_regs = Vec::new();
    if let Some(t) = &cfg.this_arg {
        args_regs.push(t.reg_num);
    }
    args_regs.extend(cfg.arg_regs.iter().map(|r| r.reg_num));
    Some(Marked {
        attr: InlineAttr {
            insns,
            root: Some(new_root),
            args_regs,
            regs_count: cfg.regs_count,
        },
        make_public,
    })
}

/// Copy an instruction with all its wrapped instructions (and the condition of a ternary) to a
/// separate arena (`copyWithoutResult` followed by unbinding of the SSA variables).
pub fn extract_tree(cfg: &MethodCfg, id: InsnId, out: &mut Vec<InsnNode>) -> InsnId {
    let src = |i: InsnId| cfg.insn(i).clone();
    let mut add = |n: InsnNode| {
        out.push(n);
        out.len() - 1
    };
    copy_tree(&src, &mut add, id, true)
}

/// `copyWithoutResult` of an instruction tree between two arenas. With `unbind` the registers
/// of the copy are not bound to variables.
pub fn copy_tree(
    src: &dyn Fn(InsnId) -> InsnNode,
    add: &mut dyn FnMut(InsnNode) -> InsnId,
    id: InsnId,
    unbind: bool,
) -> InsnId {
    let mut node = src(id);
    node.result = None;
    if unbind {
        // exception handlers belong to the code of the method
        node.catch = None;
        node.exc_handler = None;
    }
    let args = std::mem::take(&mut node.args);
    // the condition of a ternary is part of the instruction
    match &mut node.kind {
        InsnKind::Ternary { cond } => *cond = copy_tree(src, add, *cond, unbind),
        InsnKind::TernaryCond(c) => copy_condition(src, add, c, unbind),
        _ => {}
    }
    let mut new_args = Vec::with_capacity(args.len());
    for a in args {
        match a {
            InsnArg::Register(mut r) => {
                if unbind {
                    r.svar = None;
                }
                new_args.push(InsnArg::Register(r));
            }
            InsnArg::Wrapped(mut w) => {
                w.insn = copy_tree(src, add, w.insn, unbind);
                new_args.push(InsnArg::Wrapped(w));
            }
            other => new_args.push(other),
        }
    }
    node.args = new_args;
    add(node)
}

fn copy_condition(
    src: &dyn Fn(InsnId) -> InsnNode,
    add: &mut dyn FnMut(InsnNode) -> InsnId,
    cond: &mut crate::model::condition::IfCondition,
    unbind: bool,
) {
    if let Some(c) = cond.compare {
        cond.compare = Some(copy_tree(src, add, c, unbind));
    }
    for a in &mut cond.args {
        copy_condition(src, add, a, unbind);
    }
}

fn fix_visibility_of_inline_code(
    root: &Root,
    cfg: &MethodCfg,
    insn: InsnId,
    respect: bool,
    make_public: &mut Vec<MethodId>,
) -> bool {
    let n = cfg.insn(insn);
    match n.ty {
        InsnType::Invoke => {
            if let InsnKind::Invoke { mth, .. } = &n.kind {
                if let Some(call) = root.resolve_method(mth) {
                    if !respect {
                        make_public.push(call);
                    }
                }
            }
            true
        }
        InsnType::OneArg => match n.args.first() {
            Some(InsnArg::Wrapped(w)) => {
                fix_visibility_of_inline_code(root, cfg, w.insn, respect, make_public)
            }
            _ => false,
        },
        _ => matches!(&n.kind, InsnKind::Index(IndexValue::Field(_))),
    }
}

/// Make the methods public (`FixAccessModifiers.changeVisibility`).
pub fn apply_visibility(root: &mut Root, make_public: &[MethodId]) {
    for m in make_public {
        crate::passes::fix_access_modifiers::change_method_visibility(root, *m, ACC_PUBLIC);
    }
}
