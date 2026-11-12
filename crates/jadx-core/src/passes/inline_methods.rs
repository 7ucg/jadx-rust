//! `InlineMethods`: replace calls of methods marked in `MarkMethodsForInline` (synthetic accessors
//! and similar) with the code of the method.
//!
//! The pass needs the state of the other methods and classes, the driver combines the parts
//! implemented here.

use std::collections::HashMap;

use crate::method_utils::call_method;
use crate::model::cfg::MethodCfg;
use crate::model::insn::{
    iflag, BlockId, IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType,
};
use crate::model::node::{InlineAttr, MethodId, Root};
use crate::model::ssa::ArgRef;
use crate::passes::insn_utils::{register_args, replace_insn_by_ref, svar_set_name};
use crate::passes::mark_methods_for_inline::copy_tree;

/// `InsnNode.copyWithoutResult` of the inline instruction into the arena of the method.
fn copy_into(cfg: &mut MethodCfg, attr: &InlineAttr, id: InsnId) -> InsnId {
    let src = |i: InsnId| attr.insns[i].clone();
    let mut add = |n: InsnNode| cfg.add_insn(n);
    copy_tree(&src, &mut add, id, false)
}

/// Instructions of the condition of a ternary: the copy is not independent in the reference (the
/// condition object is shared with the original instruction), a replaced argument stays replaced
/// for the next inlining of the method.
type CondUpdates = Vec<(InsnId, usize, InsnArg)>;

/// Match the instructions of the copy with the instructions of the stored code; collect the
/// instructions that belong to a condition.
fn pair_insns(
    attr: &InlineAttr,
    a: InsnId,
    cfg: &MethodCfg,
    c: InsnId,
    in_cond: bool,
    map: &mut HashMap<InsnId, (InsnId, bool)>,
) {
    map.insert(c, (a, in_cond));
    for (xa, xc) in attr.insns[a].args.iter().zip(cfg.insn(c).args.iter()) {
        if let (InsnArg::Wrapped(wa), InsnArg::Wrapped(wc)) = (xa, xc) {
            pair_insns(attr, wa.insn, cfg, wc.insn, in_cond, map);
        }
    }
    match (&attr.insns[a].kind, &cfg.insn(c).kind) {
        (InsnKind::Ternary { cond: ca }, InsnKind::Ternary { cond: cc }) => {
            pair_insns(attr, *ca, cfg, *cc, true, map);
        }
        (InsnKind::TernaryCond(xa), InsnKind::TernaryCond(xc)) => {
            for (ia, ic) in xa.collect_insns().into_iter().zip(xc.collect_insns()) {
                pair_insns(attr, ia, cfg, ic, true, map);
            }
        }
        _ => {}
    }
}

/// `InlineMethods.inlineMethod`: true if the call was replaced. The instruction `insn` in
/// `block` is the call, `callee` the called method. The second value are the changes of the
/// stored code to apply.
pub fn inline_method(
    root: &Root,
    cfg: &mut MethodCfg,
    callee: MethodId,
    attr: &InlineAttr,
    block: BlockId,
    insn: InsnId,
) -> (bool, CondUpdates) {
    let mut updates = Vec::new();
    let Some(inline_root) = attr.root else {
        return (false, updates);
    };
    let inl_copy = copy_into(cfg, attr, inline_root);
    let mut map = HashMap::new();
    pair_insns(attr, inline_root, cfg, inl_copy, false, &mut map);
    if replace_regs(root, cfg, callee, attr, insn, inl_copy, &map, &mut updates) {
        let details = cfg.insn(inl_copy).details.clone();
        if let Ok(true) = replace_insn_by_ref(cfg, block, insn, inl_copy) {
            if details.is_some() {
                cfg.insns[inl_copy].details = details;
            }
            return (true, updates);
        }
    }
    let callee_name = full_name(root, callee);
    cfg.warnings
        .push(format!("Failed to inline method: {callee_name}"));
    // undo changes to insn
    let _ = cfg.unbind_insn(inl_copy);
    let _ = cfg.rebind_args(insn);
    (false, updates)
}

fn full_name(root: &Root, id: MethodId) -> String {
    let m = &root.classes[id.0].methods[id.1];
    format!(
        "{}.{}",
        root.classes[id.0].info.full_name(),
        m.info.short_id()
    )
}

#[allow(clippy::too_many_arguments)]
fn replace_regs(
    root: &Root,
    cfg: &mut MethodCfg,
    callee: MethodId,
    attr: &InlineAttr,
    insn: InsnId,
    inl_copy: InsnId,
    map: &HashMap<InsnId, (InsnId, bool)>,
    updates: &mut CondUpdates,
) -> bool {
    let callee_node = &root.classes[callee.0].methods[callee.1];
    let name = full_name(root, callee);
    if !callee_node.info.arg_types.is_empty() {
        // remap args
        let mut regs: Vec<Option<InsnArg>> = vec![None; attr.regs_count];
        for (i, &reg_num) in attr.args_regs.iter().enumerate() {
            let (Some(arg), Some(slot)) =
                (cfg.insn(insn).args.get(i), regs.get_mut(reg_num as usize))
            else {
                cfg.warnings
                    .push("Method inline failed with exception".to_string());
                return false;
            };
            *slot = Some(arg.clone());
        }
        // replace args
        for at in register_args(cfg, inl_copy) {
            let ArgRef::Arg(owner, index) = at else {
                continue;
            };
            let InsnArg::Register(r) = &cfg.insn(owner).args[index] else {
                continue;
            };
            let reg_num = r.reg_num as usize;
            let shown = format!("r{reg_num}");
            if reg_num >= regs.len() {
                cfg.warnings.push(format!(
                    "Unknown register number '{shown}' in method call: {name}"
                ));
                return false;
            }
            let Some(repl) = regs[reg_num].clone() else {
                cfg.warnings.push(format!(
                    "Not passed register '{shown}' in method call: {name}"
                ));
                return false;
            };
            let dup = cfg.duplicate_arg(&repl);
            if let Some((attr_insn, true)) = map.get(&owner) {
                // the condition is shared with the stored code
                let mut shared = dup.clone();
                if let InsnArg::Register(r) = &mut shared {
                    r.svar = None;
                }
                updates.push((*attr_insn, index, shared));
            }
            cfg.replace_arg(owner, index, dup);
        }
    }
    let result = cfg.insn(insn).result.clone();
    if let Some(res) = result {
        cfg.set_result(inl_copy, Some(res));
    } else if is_assign_needed(root, cfg, attr, insn, callee) {
        // add a fake result to make correct java expression
        let made = cfg.make_synthetic_reg_arg(callee_node.return_type.clone());
        match made {
            Ok(reg) => {
                let var = reg.svar;
                cfg.set_result(inl_copy, Some(reg));
                if let Some(v) = var {
                    let _ = svar_set_name(cfg, v, "unused");
                }
            }
            Err(_) => {
                cfg.warnings
                    .push("Method inline failed with exception".to_string());
                return false;
            }
        }
    }
    true
}

fn is_assign_needed(
    root: &Root,
    cfg: &MethodCfg,
    attr: &InlineAttr,
    insn: InsnId,
    callee: MethodId,
) -> bool {
    let parent = cfg.insn(insn);
    if parent.result.is_some() || parent.contains_flag(iflag::WRAPPED) {
        return false;
    }
    if attr
        .root
        .is_some_and(|r| attr.insns[r].ty == InsnType::Iput)
    {
        return false;
    }
    !root.classes[callee.0].methods[callee.1]
        .return_type
        .is_void()
}

fn safe_replace<T: PartialEq>(list: &mut Vec<T>, old: &T, new: T) {
    match list.iter().position(|x| x == old) {
        Some(i) => list[i] = new,
        None => list.push(new),
    }
}

/// `InlineMethods.updateUsageInfo`
pub fn update_usage_info(root: &mut Root, mth: MethodId, inlined: MethodId, attr: &InlineAttr) {
    let m = &mut root.classes[inlined.0].methods[inlined.1];
    if let Some(p) = m.use_in.iter().position(|x| *x == mth) {
        m.use_in.remove(p);
    }
    let users = m.use_in.clone();
    for u in users {
        let used = &mut root.classes[u.0].methods[u.1].used;
        if !used.contains(&inlined) {
            used.push(inlined);
        }
    }
    let Some(r) = attr.root else {
        return;
    };
    let mut order = Vec::new();
    visit_insns(attr, r, &mut order);
    for id in order {
        let insn = &attr.insns[id];
        match insn.ty {
            InsnType::Invoke | InsnType::Constructor => {
                let Some(call) = call_method(insn) else {
                    continue;
                };
                if let Some(node) = root.resolve_method(call) {
                    let list = &mut root.classes[node.0].methods[node.1].use_in;
                    safe_replace(list, &inlined, mth);
                    let users = list.clone();
                    for u in users {
                        let used = &mut root.classes[u.0].methods[u.1].used;
                        if !used.contains(&node) {
                            used.push(node);
                        }
                    }
                    replace_cls_usage(root, mth, inlined, node.0);
                }
            }
            InsnType::Iget | InsnType::Iput | InsnType::Sput | InsnType::Sget => {
                let InsnKind::Index(IndexValue::Field(f)) = &insn.kind else {
                    continue;
                };
                if let Some((c, fi)) = root.resolve_field_id(f) {
                    let list = &mut root.classes[c].fields[fi].use_in;
                    safe_replace(list, &inlined, mth);
                    replace_cls_usage(root, mth, inlined, c);
                }
            }
            _ => {}
        }
    }
}

fn visit_insns(attr: &InlineAttr, id: InsnId, out: &mut Vec<InsnId>) {
    out.push(id);
    for a in &attr.insns[id].args {
        if let InsnArg::Wrapped(w) = a {
            visit_insns(attr, w.insn, out);
        }
    }
}

fn replace_cls_usage(root: &mut Root, mth: MethodId, inlined: MethodId, parent_class: usize) {
    let cls = &mut root.classes[parent_class];
    safe_replace(&mut cls.use_in_mth, &inlined, mth);
    safe_replace(&mut cls.use_in, &inlined.0, mth.0);
}
