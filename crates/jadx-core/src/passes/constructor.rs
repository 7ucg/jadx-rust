//! `ConstructorVisitor`: replace `invoke <init>` with constructor instructions.

use crate::model::cfg::MethodCfg;
use crate::model::info::{type_signature, ClassInfo, MethodInfo};
use crate::model::insn::{
    iflag, BlockId, CallType, IndexValue, InsnArg, InsnId, InsnKind, InsnNode, InsnType,
    InvokeType, RegisterArg,
};
use crate::model::node::Root;
use crate::model::ssa::ArgRef;
use crate::passes::block_utils;

const ACC_SYNTHETIC: u32 = 0x1000;

/// The method being processed, within its root.
pub struct MethodEnv<'a> {
    pub root: &'a Root,
    pub cls: usize,
    pub mth: usize,
}

impl MethodEnv<'_> {
    fn class(&self) -> &crate::model::node::ClassNode {
        &self.root.classes[self.cls]
    }
    fn method(&self) -> &crate::model::node::MethodNode {
        &self.root.classes[self.cls].methods[self.mth]
    }
}

/// `InsnRemover` bound to one block.
#[derive(Default)]
struct Remover {
    to_remove: Vec<InsnId>,
}

impl Remover {
    fn add_and_unbind(&mut self, cfg: &mut MethodCfg, insn: InsnId) -> Result<(), String> {
        self.to_remove.push(insn);
        cfg.unbind_insn(insn)
    }

    fn add_without_unbind(&mut self, insn: InsnId) {
        self.to_remove.push(insn);
    }

    fn perform(&mut self, cfg: &mut MethodCfg, block: BlockId) -> Result<(), String> {
        if self.to_remove.is_empty() {
            return Ok(());
        }
        for &i in &self.to_remove {
            cfg.unbind_all_args(i);
        }
        for &i in &self.to_remove {
            cfg.unbind_result(i)?;
            cfg.insns[i].flags |= iflag::DONT_GENERATE;
        }
        for &rem in &self.to_remove {
            if let Some(p) = cfg.arena[block].insns.iter().position(|i| *i == rem) {
                cfg.arena[block].insns.remove(p);
            }
        }
        self.to_remove.clear();
        Ok(())
    }
}

/// Returns `true` if any invoke was replaced (the caller then runs the move inlining again).
pub fn replace_invoke(cfg: &mut MethodCfg, env: &MethodEnv) -> Result<bool, String> {
    let mut replaced = false;
    for bid in cfg.blocks.clone() {
        let mut remover = Remover::default();
        let size = cfg.block(bid).insns.len();
        for i in 0..size {
            let iid = cfg.block(bid).insns[i];
            if cfg.insn(iid).ty == InsnType::Invoke {
                replaced |= process_invoke(cfg, env, bid, i, &mut remover)?;
            }
        }
        remover.perform(cfg, bid)?;
    }
    Ok(replaced)
}

fn instance_arg_reg(cfg: &MethodCfg, inv: InsnId) -> Option<RegisterArg> {
    match cfg.insn(inv).args.first() {
        Some(InsnArg::Register(r)) => Some(r.clone()),
        _ => None,
    }
}

/// `searchInstanceType`
fn search_instance_type(cfg: &MethodCfg, inv: InsnId) -> Option<crate::model::types::ArgType> {
    let InsnKind::Invoke { invoke_type, .. } = &cfg.insn(inv).kind else {
        return None;
    };
    if *invoke_type == InvokeType::Static {
        return None;
    }
    let reg = instance_arg_reg(cfg, inv)?;
    let assign = cfg.svars[reg.svar?].assign.parent_insn()?;
    let a = cfg.insn(assign);
    if a.ty != InsnType::NewInstance {
        return None;
    }
    match &a.kind {
        InsnKind::Index(IndexValue::Type(t)) => Some(t.clone()),
        _ => None,
    }
}

fn get_call_type(
    env: &MethodEnv,
    class_type: &ClassInfo,
    call_mth: &MethodInfo,
    instance: Option<&InsnArg>,
) -> CallType {
    let is_this = matches!(instance, Some(InsnArg::Register(r)) if r.is_this);
    if !is_this {
        return CallType::Constructor;
    }
    if *class_type != env.class().info {
        return CallType::Super;
    }
    if call_mth.short_id() == env.method().info.short_id() {
        return CallType::SelfCall;
    }
    CallType::This
}

fn process_invoke(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    block: BlockId,
    index: usize,
    remover: &mut Remover,
) -> Result<bool, String> {
    let inv = cfg.block(block).insns[index];
    let InsnKind::Invoke { mth: call_mth, .. } = cfg.insn(inv).kind.clone() else {
        return Ok(false);
    };
    if !call_mth.is_constructor() {
        return Ok(false);
    }
    let mut call_mth = call_mth;
    if let Some(inst_type) = search_instance_type(cfg, inv) {
        if inst_type != *call_mth.decl_class.arg_type() {
            let inst_cls = env.root.ctx.class_info(&inst_type)?;
            call_mth = MethodInfo {
                decl_class: inst_cls,
                ..call_mth
            };
        }
    }
    let inv_args = cfg.insn(inv).args.clone();
    let call_type = get_call_type(env, &call_mth.decl_class, &call_mth, inv_args.first());
    let co_args: Vec<InsnArg> = inv_args.iter().skip(1).cloned().collect();
    if can_remove_constructor(env, call_type, co_args.len(), &call_mth) {
        remover.add_and_unbind(cfg, inv)?;
        return Ok(false);
    }
    let mut co = InsnNode::new(
        InsnType::Constructor,
        InsnKind::Constructor {
            call_mth: call_mth.clone(),
            call_type,
        },
    );
    co.args = co_args;
    let co_id = cfg.add_insn(co);
    // `co.inheritMetadata(inv)`: before the new-instance assign chain is folded in below, so a
    // debug line on the still-live `invoke-direct` is picked up first (`addSourceLineFrom` only
    // fills a still-unset source line, so this does not take priority over the `new-instance`
    // line inherited next).
    cfg.inherit_metadata(co_id, inv);
    let Some(mut instance_arg) = instance_arg_reg(cfg, inv) else {
        return Err("constructor call without instance register".into());
    };
    let inst_var = instance_arg
        .svar
        .ok_or_else(|| "instance register without SSA variable".to_string())?;
    cfg.svar_remove_use(inst_var, ArgRef::Arg(inv, 0));
    if call_type == CallType::Constructor {
        if let Some(assign) = cfg.svars[inst_var].assign.parent_insn() {
            if cfg.insn(assign).ty == InsnType::Constructor {
                instance_arg = insert_phi_insn(cfg, block, &instance_arg, assign)?;
            } else if let Some(new_inst) =
                remove_assign_chain(cfg, assign, remover, InsnType::NewInstance)?
            {
                // `co.inheritMetadata(newInstInsn)`: the debug line table usually marks the
                // `new-instance` offset rather than the `invoke-direct` one, so this is where a
                // constructor call most often actually gets its source line from.
                cfg.inherit_metadata(co_id, new_inst);
                cfg.insns[new_inst].flags |= iflag::REMOVE;
                remover.add_without_unbind(new_inst);
            }
        }
        cfg.set_result(co_id, Some(instance_arg.clone()));
    }
    cfg.rebind_args(co_id)?;
    match process_constructor(cfg, env, co_id)? {
        Some(replace) => {
            remover.add_and_unbind(cfg, co_id)?;
            replace_insn(cfg, block, index, replace)?;
        }
        None => replace_insn(cfg, block, index, co_id)?,
    }
    Ok(true)
}

/// `BlockUtils.replaceInsn(mth, block, i, insn)`
fn replace_insn(
    cfg: &mut MethodCfg,
    block: BlockId,
    index: usize,
    new: InsnId,
) -> Result<(), String> {
    let prev = cfg.block(block).insns[index];
    let prev_copy = cfg.insns[prev].clone();
    cfg.insns[new].copy_attributes_from(&prev_copy);
    cfg.inherit_metadata(new, prev);
    cfg.insns[new].offset = prev_copy.offset;
    cfg.arena[block].insns[index] = new;
    let same_result = match (&cfg.insns[new].result, &prev_copy.result) {
        (Some(a), Some(b)) => a.reg_num == b.reg_num && a.svar == b.svar,
        _ => false,
    };
    if same_result {
        cfg.unbind_all_args(prev);
    } else {
        cfg.unbind_insn(prev)?;
    }
    cfg.rebind_args(new)
}

/// `MethodNode.isDefaultConstructor`
fn is_default_constructor(env: &MethodEnv) -> bool {
    let m = env.method();
    if !m.is_constructor() {
        return false;
    }
    let cls = env.class();
    let mut default_arg_count = 0;
    // workaround for non-static inner class constructor, that has synthetic argument
    if cls.info.is_inner() && cls.access & crate::model::node::ACC_STATIC == 0 {
        if let Some(outer) = env.root.parent_index(env.cls) {
            if m.arg_types.first() == Some(env.root.classes[outer].info.arg_type()) {
                default_arg_count = 1;
            }
        }
    }
    m.arg_types.len() == default_arg_count
}

/// `canRemoveConstructor`
fn can_remove_constructor(
    env: &MethodEnv,
    call_type: CallType,
    args_count: usize,
    call_mth: &MethodInfo,
) -> bool {
    if call_type == CallType::Super && args_count == 0 {
        return true;
    }
    if call_type == CallType::This && args_count == 0 {
        let sid = call_mth.short_id();
        let def_co = env
            .class()
            .methods
            .iter()
            .find(|m| m.info.short_id() == sid);
        if def_co.is_none_or(|m| m.code_offset.is_none()) {
            return true;
        }
    }
    env.class().is_anonymous() && is_default_constructor(env) && call_type == CallType::Super
}

/// `processConstructor`: replace a call of a synthetic constructor (all arguments null) by the
/// default constructor.
fn process_constructor(
    cfg: &mut MethodCfg,
    env: &MethodEnv,
    co: InsnId,
) -> Result<Option<InsnId>, String> {
    let (call_mth, call_type) = match &cfg.insn(co).kind {
        InsnKind::Constructor {
            call_mth,
            call_type,
        } => (call_mth.clone(), *call_type),
        _ => return Ok(None),
    };
    let Some((ci, mi)) = env.root.resolve_method(&call_mth) else {
        return Ok(None);
    };
    let target = &env.root.classes[ci].methods[mi];
    if target.access & ACC_SYNTHETIC == 0 || !all_args_null(cfg.insn(co)) {
        return Ok(None);
    }
    let Some(class_idx) = env.root.class_index(&env.root.classes[ci].info.raw_name()) else {
        return Ok(None);
    };
    let class_node = &env.root.classes[class_idx];
    let Some(instance_arg) = cfg.insn(co).result.clone() else {
        return Ok(None);
    };
    let pass_this = instance_arg.is_this;
    let ctr_id = format!(
        "<init>({})V",
        if pass_this {
            type_signature(&instance_arg.init_type)
        } else {
            String::new()
        }
    );
    let Some(def_idx) = class_node
        .methods
        .iter()
        .position(|m| m.info.short_id() == ctr_id)
    else {
        return Ok(None);
    };
    let def_ctr = &class_node.methods[def_idx];
    if (class_idx, def_idx) == (ci, mi) || def_ctr.access & ACC_SYNTHETIC != 0 {
        return Ok(None);
    }
    let new_insn = InsnNode::new(
        InsnType::Constructor,
        InsnKind::Constructor {
            call_mth: def_ctr.info.clone(),
            call_type,
        },
    );
    let new_id = cfg.add_insn(new_insn);
    cfg.set_result(new_id, Some(instance_arg));
    cfg.inherit_metadata(new_id, co);
    Ok(Some(new_id))
}

fn all_args_null(insn: &InsnNode) -> bool {
    insn.args
        .iter()
        .all(|a| matches!(a, InsnArg::Literal(l) if l.literal == 0))
}

/// `removeAssignChain`
fn remove_assign_chain(
    cfg: &mut MethodCfg,
    insn: InsnId,
    remover: &mut Remover,
    target: InsnType,
) -> Result<Option<InsnId>, String> {
    let ty = cfg.insn(insn).ty;
    if ty == target {
        return Ok(Some(insn));
    }
    if cfg.insn(insn).attrs_empty() {
        remover.add_without_unbind(insn);
    } else {
        replace_with_nop(cfg, insn)?;
    }
    if ty == InsnType::Move {
        let next = match cfg.insn(insn).args.first() {
            Some(InsnArg::Register(r)) => r.svar.and_then(|v| cfg.svars[v].assign.parent_insn()),
            _ => None,
        };
        if let Some(n) = next {
            return remove_assign_chain(cfg, n, remover, target);
        }
    }
    Ok(None)
}

/// `BlockUtils.replaceInsn(mth, insn, new InsnNode(NOP, 0))`
fn replace_with_nop(cfg: &mut MethodCfg, insn: InsnId) -> Result<(), String> {
    let nop = cfg.add_insn(InsnNode::plain(InsnType::Nop));
    let Some(block) = cfg.block_by_insn(insn) else {
        return Ok(());
    };
    let Some(idx) = cfg.block(block).insns.iter().position(|i| *i == insn) else {
        return Ok(());
    };
    replace_insn(cfg, block, idx, nop)
}

/// `insertPhiInsn`: the instance is already constructed by another constructor call.
fn insert_phi_insn(
    cfg: &mut MethodCfg,
    cur_block: BlockId,
    inst_arg: &RegisterArg,
    other_ctr: InsnId,
) -> Result<RegisterArg, String> {
    let Some(other_block) = cfg.block_by_insn(other_ctr) else {
        return Err("Block not found by insn".into());
    };
    let cross = block_utils::get_path_cross_pair(cfg, cur_block, other_block);
    let (new_res_at, _new_var) = cfg.duplicate_with_new_svar(inst_arg);
    let new_res = cfg.reg(new_res_at).clone();
    let Some(cross) = cross else {
        let path_blocks = block_utils::collect_all_successors(cfg, cur_block, true);
        let var = inst_arg.svar.ok_or("no SSA var")?;
        // the reference iterates the live use list while it is modified
        let mut cursor = 0;
        while cursor < cfg.svars[var].uses.len() {
            let use_reg = cfg.svars[var].uses[cursor];
            cursor += 1;
            let Some(parent) = use_reg.parent_insn() else {
                continue;
            };
            let in_path = cfg.insn(parent).ty == InsnType::Phi
                || path_blocks
                    .iter()
                    .any(|b| cfg.block(*b).insns.contains(&parent));
            if in_path {
                if let ArgRef::Arg(p, k) = use_reg {
                    cfg.replace_arg(p, k, InsnArg::Register(new_res.clone()));
                }
                if cursor != cfg.svars[var].uses.len() {
                    return Err("ConcurrentModificationException".into());
                }
            }
        }
        return Ok(new_res);
    };
    let use_arg = cfg
        .insn(other_ctr)
        .result
        .clone()
        .ok_or("constructor without result")?;
    let (other_res_at, _) = cfg.duplicate_with_new_svar(&use_arg);
    let other_res = cfg.reg(other_res_at).clone();
    let phi = crate::passes::ssa::add_phi_attr(cfg, cross, use_arg.reg_num);
    cfg.set_result(phi, Some(use_arg.clone()));
    let b1 = block_utils::get_prev_block_on_path(cfg, cross, cur_block).ok_or("no path block")?;
    let b2 = block_utils::get_prev_block_on_path(cfg, cross, other_block).ok_or("no path block")?;
    cfg.phi_bind_arg_reg(phi, new_res.clone(), b1)?;
    cfg.phi_bind_arg_reg(phi, other_res.clone(), b2)?;
    cfg.rebind_args(phi)?;
    cfg.set_result(other_ctr, Some(other_res));
    cfg.rebind_args(other_ctr)?;
    Ok(new_res)
}
