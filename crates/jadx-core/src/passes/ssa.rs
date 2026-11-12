//! SSA construction (`SSATransform`, `LiveVarAnalysis`, `RenameState`).

use std::collections::VecDeque;

use crate::model::bitset::BitSet;
use crate::model::cfg::MethodCfg;
use crate::model::insn::{iflag, BlockId, InsnArg, InsnId, InsnNode, InsnType, RegisterArg};
use crate::model::ssa::{ArgRef, SsaId};
use crate::model::types::ArgType;

/// `MethodNode.initArguments`: `this` and argument registers placed at the end of the register
/// frame (`argsStartReg` is not available for DEX, so the start is derived from the sizes).
pub fn init_arguments(
    cfg: &mut MethodCfg,
    regs_count: usize,
    is_static: bool,
    this_type: &ArgType,
    arg_types: &[ArgType],
) {
    cfg.regs_count = regs_count;
    let mut pos = regs_count as i64;
    for a in arg_types {
        pos -= a.reg_count() as i64;
    }
    if !is_static {
        pos -= 1;
    }
    let mut pos = pos.max(0) as u32;
    cfg.this_arg = None;
    if !is_static {
        let mut arg = RegisterArg::new(pos, this_type.clone());
        pos += 1;
        arg.is_this = true;
        arg.immutable_type = true;
        cfg.this_arg = Some(arg);
    }
    cfg.arg_regs.clear();
    for t in arg_types {
        let mut arg = RegisterArg::new(pos, t.clone());
        arg.method_arg = true;
        arg.immutable_type = true;
        cfg.arg_regs.push(arg);
        pos += t.reg_count();
    }
}

fn bitset_eq(a: &BitSet, b: &BitSet) -> bool {
    a.iter().eq(b.iter())
}

/// `LiveVarAnalysis`
struct LiveVarAnalysis {
    live_in: Vec<BitSet>,
    assign_blocks: Vec<BitSet>,
}

impl LiveVarAnalysis {
    fn run(cfg: &MethodCfg) -> Result<Self, String> {
        let bb_count = cfg.blocks.len();
        let regs = cfg.regs_count;
        let mut uses = vec![BitSet::new(); bb_count];
        let mut defs = vec![BitSet::new(); bb_count];
        let mut assign_blocks = vec![BitSet::new(); regs];
        for &bid in &cfg.blocks {
            let block = cfg.block(bid);
            let id = block.pos;
            for &iid in &block.insns {
                let insn = cfg.insn(iid);
                for arg in &insn.args {
                    if let InsnArg::Register(r) = arg {
                        let reg = r.reg_num as usize;
                        if reg >= regs {
                            return Err(format!("register {reg} out of range ({regs})"));
                        }
                        if !defs[id].get(reg) {
                            uses[id].set(reg);
                        }
                    }
                }
                if let Some(res) = &insn.result {
                    let reg = res.reg_num as usize;
                    if reg >= regs {
                        return Err(format!("register {reg} out of range ({regs})"));
                    }
                    defs[id].set(reg);
                    assign_blocks[reg].set(id);
                }
            }
        }
        let mut live_in = vec![BitSet::new(); bb_count];
        let limit = bb_count * 10;
        let mut k = 0;
        loop {
            let mut changed = false;
            for &bid in &cfg.blocks {
                let block = cfg.block(bid);
                let id = block.pos;
                let mut new_in = BitSet::new();
                for s in &block.succs {
                    new_in.or(&live_in[cfg.block(*s).pos]);
                }
                new_in.and_not(&defs[id]);
                new_in.or(&uses[id]);
                if !bitset_eq(&live_in[id], &new_in) {
                    changed = true;
                    live_in[id] = new_in;
                }
            }
            k += 1;
            if k - 1 > limit {
                return Err(format!(
                    "Live variable analysis reach iterations limit, blocks count: {bb_count}"
                ));
            }
            if !changed {
                break;
            }
        }
        Ok(LiveVarAnalysis {
            live_in,
            assign_blocks,
        })
    }

    fn is_live(&self, block_pos: usize, reg: usize) -> bool {
        block_pos < self.live_in.len() && self.live_in[block_pos].get(reg)
    }
}

/// `SSATransform.process`
pub fn ssa_transform(cfg: &mut MethodCfg) -> Result<(), String> {
    if !cfg.svar_list.is_empty() {
        return Ok(());
    }
    let la = LiveVarAnalysis::run(cfg)?;
    for reg in 0..cfg.regs_count {
        place_phi(cfg, reg, &la)?;
    }
    rename_variables(cfg)?;
    fix_last_assign_in_try(cfg)?;
    remove_blocker_insns(cfg)?;
    try_to_fix_useless_phi(cfg)?;
    mark_this_args(cfg);
    hide_phi_insns(cfg);
    remove_unused_invoke_results(cfg);
    Ok(())
}

fn place_phi(cfg: &mut MethodCfg, reg: usize, la: &LiveVarAnalysis) -> Result<(), String> {
    let blocks_count = cfg.blocks.len();
    let mut has_phi = BitSet::new();
    let mut processed = BitSet::new();
    let mut work: VecDeque<BlockId> = VecDeque::new();
    for id in la.assign_blocks[reg].iter() {
        processed.set(id);
        work.push_back(cfg.blocks[id]);
    }
    while let Some(block) = work.pop_front() {
        let frontier = cfg
            .block(block)
            .dom_frontier
            .clone()
            .ok_or_else(|| "dominance frontier not set".to_string())?;
        for id in frontier.iter() {
            if id >= blocks_count {
                continue;
            }
            if !has_phi.get(id) && la.is_live(id, reg) {
                let df = cfg.blocks[id];
                let phi = add_phi(cfg, df, reg as u32);
                cfg.arena[df].insns.insert(0, phi);
                has_phi.set(id);
                if !processed.get(id) {
                    processed.set(id);
                    work.push_back(df);
                }
            }
        }
    }
    Ok(())
}

/// `SSATransform.addPhi`
fn add_phi(cfg: &mut MethodCfg, block: BlockId, reg: u32) -> InsnId {
    let mut phi = MethodCfg::new_phi(reg);
    phi.offset = cfg.block(block).start_offset;
    let id = cfg.add_insn(phi);
    cfg.arena[block]
        .phi_list
        .get_or_insert_with(Vec::new)
        .push(id);
    id
}

/// `RenameState`
#[derive(Clone)]
struct RenameState {
    block: BlockId,
    vars: Vec<Option<SsaId>>,
}

impl RenameState {
    fn start_var(&mut self, cfg: &mut MethodCfg, versions: &mut [u32], reg: ArgRef) -> SsaId {
        let reg_num = cfg.reg(reg).reg_num;
        let version = versions[reg_num as usize];
        versions[reg_num as usize] += 1;
        let var = cfg.make_new_svar(reg_num, version, reg);
        self.vars[reg_num as usize] = Some(var);
        var
    }
}

fn rename_variables(cfg: &mut MethodCfg) -> Result<(), String> {
    let regs = cfg.regs_count;
    let mut versions = vec![0u32; regs];
    let mut init = RenameState {
        block: cfg.enter,
        vars: vec![None; regs],
    };
    if cfg.this_arg.is_some() {
        init.start_var(cfg, &mut versions, ArgRef::This);
    }
    for i in 0..cfg.arg_regs.len() {
        init.start_var(cfg, &mut versions, ArgRef::Method(i));
    }
    // initPhiInEnterBlock
    if let Some(list) = cfg.block(init.block).phi_list.clone() {
        for phi in list {
            bind_phi_arg(cfg, &init, phi)?;
        }
    }
    let mut stack = vec![init];
    while let Some(mut state) = stack.pop() {
        rename_vars_in_block(cfg, &mut state, &mut versions)?;
        for dominated in cfg.block(state.block).dominates_on.clone() {
            let mut copy = state.clone();
            copy.block = dominated;
            stack.push(copy);
        }
    }
    Ok(())
}

fn rename_vars_in_block(
    cfg: &mut MethodCfg,
    state: &mut RenameState,
    versions: &mut [u32],
) -> Result<(), String> {
    let block = state.block;
    let regs = cfg.regs_count;
    for iid in cfg.block(block).insns.clone() {
        if cfg.insn(iid).ty != InsnType::Phi {
            for k in 0..cfg.insn(iid).args.len() {
                let reg_num = match &cfg.insn(iid).args[k] {
                    InsnArg::Register(r) => r.reg_num as usize,
                    _ => continue,
                };
                if reg_num >= regs {
                    return Err(format!("register {reg_num} out of range ({regs})"));
                }
                let var = match state.vars[reg_num] {
                    Some(v) => v,
                    None => {
                        let desc = format!(
                            "Not initialized variable reg: {reg_num}, insn: {} @{:x}, block: {}",
                            cfg.insn(iid).ty.name(),
                            cfg.insn(iid).offset,
                            cfg.block(block).cid
                        );
                        cfg.warnings.push(desc);
                        state.start_var(cfg, versions, ArgRef::Arg(iid, k))
                    }
                };
                cfg.svar_use(var, ArgRef::Arg(iid, k));
            }
        }
        if cfg.insn(iid).result.is_some() {
            let reg_num = cfg.reg(ArgRef::Result(iid)).reg_num as usize;
            if reg_num >= regs {
                return Err(format!("register {reg_num} out of range ({regs})"));
            }
            state.start_var(cfg, versions, ArgRef::Result(iid));
        }
    }
    for s in cfg.block(block).succs.clone() {
        if let Some(list) = cfg.block(s).phi_list.clone() {
            for phi in list {
                bind_phi_arg(cfg, state, phi)?;
            }
        }
    }
    Ok(())
}

fn bind_phi_arg(cfg: &mut MethodCfg, state: &RenameState, phi: InsnId) -> Result<(), String> {
    let reg_num = cfg.reg(ArgRef::Result(phi)).reg_num as usize;
    let Some(var) = state.vars[reg_num] else {
        return Ok(());
    };
    let arg = cfg.phi_bind_arg(phi, state.block)?;
    cfg.svar_use(var, arg);
    cfg.svar_add_used_in_phi(var, phi);
    Ok(())
}

fn fix_last_assign_in_try(cfg: &mut MethodCfg) -> Result<(), String> {
    for bid in cfg.blocks.clone() {
        let Some(list) = cfg.block(bid).phi_list.clone() else {
            continue;
        };
        let Some(handler) = cfg.block(bid).exc_handler else {
            continue;
        };
        for phi in list {
            let mut k = 0;
            while k < cfg.insn(phi).args.len() {
                let assign = cfg_arg_assign_insn(cfg, ArgRef::Arg(phi, k));
                if should_skip_insn_result(cfg, assign, handler) {
                    cfg.phi_remove_arg(phi, k);
                } else {
                    k += 1;
                }
            }
            if cfg.insn(phi).args.is_empty() {
                return Err("PHI empty after try-catch fix!".into());
            }
        }
    }
    Ok(())
}

/// `RegisterArg.getAssignInsn`
fn cfg_arg_assign_insn(cfg: &MethodCfg, arg: ArgRef) -> Option<InsnId> {
    let v = cfg.reg(arg).svar?;
    cfg.svars[v].assign.parent_insn()
}

fn should_skip_insn_result(cfg: &MethodCfg, insn: Option<InsnId>, handler: usize) -> bool {
    let Some(insn) = insn else { return false };
    let n = cfg.insn(insn);
    if n.result.is_some() && n.contains_flag(iflag::TRY_LEAVE) {
        let catch = match &n.catch {
            Some(c) => Some(c.clone()),
            None => cfg
                .block_by_insn(insn)
                .and_then(|b| cfg.block(b).catch.clone()),
        };
        return catch.is_some_and(|c| c.contains(&handler));
    }
    false
}

fn remove_blocker_insns(cfg: &mut MethodCfg) -> Result<bool, String> {
    let mut removed = false;
    for bid in cfg.blocks.clone() {
        let Some(list) = cfg.block(bid).phi_list.clone() else {
            continue;
        };
        for phi in list {
            let mut i = 0;
            while i < cfg.insn(phi).args.len() {
                let parent = cfg_arg_assign_insn(cfg, ArgRef::Arg(phi, i));
                if let Some(p) = parent {
                    if cfg.insn(p).contains_flag(iflag::REMOVE) {
                        cfg.phi_remove_arg(phi, i);
                        cfg.remove_insn_in_block(bid, p)?;
                        removed = true;
                    }
                }
                i += 1;
            }
        }
    }
    Ok(removed)
}

fn try_to_fix_useless_phi(cfg: &mut MethodCfg) -> Result<(), String> {
    let max_tries = cfg.svar_list.len() * 2;
    let mut k = 0;
    while fix_useless_phi(cfg)? {
        k += 1;
        if k - 1 > max_tries {
            return Err("Phi nodes fix limit reached!".into());
        }
    }
    Ok(())
}

fn fix_useless_phi(cfg: &mut MethodCfg) -> Result<bool, String> {
    let mut changed = false;
    let mut to_remove: Vec<InsnId> = Vec::new();
    for v in cfg.svar_list.clone() {
        if cfg.svars[v].use_count() == 0 {
            if let Some(a) = cfg.svars[v].assign.parent_insn() {
                if cfg.insn(a).ty == InsnType::Phi {
                    to_remove.push(a);
                    changed = true;
                }
            }
        }
    }
    for bid in cfg.blocks.clone() {
        let Some(list) = cfg.block(bid).phi_list.clone() else {
            continue;
        };
        let mut keep = Vec::with_capacity(list.len());
        for phi in list {
            if fix_phi_with_same_args(cfg, bid, phi)? {
                changed = true;
            } else {
                keep.push(phi);
            }
        }
        cfg.arena[bid].phi_list = Some(keep);
    }
    remove_phi_list(cfg, &to_remove)?;
    Ok(changed)
}

fn fix_phi_with_same_args(
    cfg: &mut MethodCfg,
    block: BlockId,
    phi: InsnId,
) -> Result<bool, String> {
    let argc = cfg.insn(phi).args.len();
    if argc == 0 {
        // the reference tries to detach uses of the result inside other phis here, but removes
        // from the wrong instruction, which has no effect
        cfg.remove_insn_in_block(block, phi)?;
        return Ok(true);
    }
    let all_same = argc == 1 || is_same_args(cfg, phi);
    if all_same {
        return replace_phi_with_move(cfg, block, phi);
    }
    if let Some(same_var) = is_same_move(cfg, phi) {
        let mut dup = cfg.reg(cfg.svars[same_var].assign).clone();
        dup.svar = Some(same_var);
        if inline_phi_insn(cfg, block, phi, dup)? {
            for k in 0..cfg.insn(phi).args.len() {
                if let Some(mv) = cfg_arg_assign_insn(cfg, ArgRef::Arg(phi, k)) {
                    cfg.insns[mv].flags |= iflag::REMOVE;
                    cfg.remove_insn(mv)?;
                }
            }
            return Ok(true);
        }
    }
    Ok(false)
}

fn phi_arg_var(cfg: &MethodCfg, phi: InsnId, i: usize) -> Option<SsaId> {
    cfg.reg(ArgRef::Arg(phi, i)).svar
}

fn is_same_args(cfg: &MethodCfg, phi: InsnId) -> bool {
    let mut var: Option<Option<SsaId>> = None;
    for i in 0..cfg.insn(phi).args.len() {
        let v = phi_arg_var(cfg, phi, i);
        match var {
            None => var = Some(v),
            Some(first) => {
                if first != v {
                    return false;
                }
            }
        }
    }
    true
}

fn is_same_move(cfg: &MethodCfg, phi: InsnId) -> Option<SsaId> {
    let mut var: Option<SsaId> = None;
    for i in 0..cfg.insn(phi).args.len() {
        let v = phi_arg_var(cfg, phi, i)?;
        if cfg.svars[v].use_count() != 1 {
            return None;
        }
        let assign = cfg.svars[v].assign.parent_insn()?;
        let mv = cfg.insn(assign);
        if mv.ty != InsnType::Move {
            return None;
        }
        let InsnArg::Register(src) = &mv.args[0] else {
            return None;
        };
        let move_var = src.svar?;
        match var {
            None => var = Some(move_var),
            Some(first) => {
                if first != move_var {
                    return None;
                }
            }
        }
    }
    var
}

fn remove_phi_list(cfg: &mut MethodCfg, to_remove: &[InsnId]) -> Result<(), String> {
    for bid in cfg.blocks.clone() {
        let Some(mut list) = cfg.block(bid).phi_list.clone() else {
            continue;
        };
        for &phi in to_remove {
            if let Some(p) = list.iter().position(|i| *i == phi) {
                list.remove(p);
                cfg.arena[bid].phi_list = Some(list.clone());
                for k in 0..cfg.insn(phi).args.len() {
                    if let InsnArg::Register(r) = &cfg.insn(phi).args[k] {
                        if let Some(v) = r.svar {
                            cfg.svar_remove_used_in_phi(v, phi);
                        }
                    }
                }
                cfg.remove_insn_in_block(bid, phi)?;
            }
        }
        if list.is_empty() {
            cfg.arena[bid].phi_list = None;
        } else {
            cfg.arena[bid].phi_list = Some(list);
        }
    }
    Ok(())
}

fn replace_phi_with_move(cfg: &mut MethodCfg, block: BlockId, phi: InsnId) -> Result<bool, String> {
    let Some(phi_index) = cfg.block(block).insns.iter().position(|i| *i == phi) else {
        return Ok(false);
    };
    let assign = cfg.reg(ArgRef::Result(phi)).svar;
    let arg0 = ArgRef::Arg(phi, 0);
    let arg_var = cfg.reg(arg0).svar;
    if let Some(av) = arg_var {
        cfg.svar_remove_use(av, arg0);
        cfg.svar_remove_used_in_phi(av, phi);
    }
    let inline_arg = match &cfg.insn(phi).args[0] {
        InsnArg::Register(r) => r.clone(),
        _ => return Ok(false),
    };
    if inline_phi_insn(cfg, block, phi, inline_arg.clone())? {
        cfg.arena[block].insns.remove(phi_index);
    } else {
        if let Some(a) = assign {
            cfg.svar_remove_used_in_phi(a, phi);
        }
        let mut m = InsnNode::plain(InsnType::Move);
        m.flags |= iflag::SYNTHETIC;
        m.result = cfg.insns[phi].result.take();
        m.args.push(InsnArg::Register(inline_arg));
        let mid = cfg.add_insn(m);
        if let Some(a) = assign {
            cfg.svars[a].assign = ArgRef::Result(mid);
        }
        let v = cfg
            .reg(ArgRef::Arg(mid, 0))
            .svar
            .ok_or_else(|| "phi argument without SSA variable".to_string())?;
        cfg.svar_use(v, ArgRef::Arg(mid, 0));
        cfg.arena[block].insns[phi_index] = mid;
    }
    Ok(true)
}

fn inline_phi_insn(
    cfg: &mut MethodCfg,
    block: BlockId,
    phi: InsnId,
    inline_arg: RegisterArg,
) -> Result<bool, String> {
    let Some(res_var) = cfg.reg(ArgRef::Result(phi)).svar else {
        return Ok(false);
    };
    let Some(inline_var) = inline_arg.svar else {
        return Ok(false);
    };
    let uses = cfg.svars[res_var].uses.clone();
    // the reference places one shared argument object into every use: only the last
    // placement stays registered as a use
    let mut placed: Option<ArgRef> = None;
    for use_arg in uses {
        let Some(use_insn) = use_arg.parent_insn() else {
            return Ok(false);
        };
        if use_insn == phi {
            return Ok(false);
        }
        if cfg.reg(use_arg).reg_num == inline_arg.reg_num {
            cfg.svar_remove_use(res_var, use_arg);
            cfg.svar_use(inline_var, use_arg);
        } else {
            let ArgRef::Arg(_, k) = use_arg else {
                return Ok(false);
            };
            let in_phi = cfg.insn(use_insn).ty == InsnType::Phi;
            if in_phi {
                cfg.svar_add_used_in_phi(inline_var, use_insn);
            }
            if let Some(p) = placed.take() {
                cfg.svar_remove_use(inline_var, p);
            }
            cfg.svar_remove_use(res_var, use_arg);
            cfg.insns[use_insn].args[k] = InsnArg::Register(inline_arg.clone());
            cfg.svar_use(inline_var, use_arg);
            if in_phi {
                cfg.svar_update_used_in_phi_list(res_var);
            }
            placed = Some(use_arg);
        }
    }
    if cfg.block(block).exc_handler.is_some() {
        if let Some(a) = cfg.svars[inline_var].assign.parent_insn() {
            if !matches!(
                cfg.insn(a).ty,
                InsnType::Const | InsnType::ConstStr | InsnType::ConstClass
            ) {
                cfg.insns[a].flags |= iflag::DONT_INLINE;
            }
        }
    }
    cfg.unbind_insn(phi)?;
    Ok(true)
}

fn mark_this_args(cfg: &mut MethodCfg) {
    if cfg.this_arg.is_some() {
        mark_this_arg(cfg, ArgRef::This);
    }
}

/// `markThisArgs(RegisterArg)`
fn mark_this_arg(cfg: &mut MethodCfg, arg: ArgRef) {
    mark_one_arg_as_this(cfg, arg);
    let Some(var) = cfg.reg(arg).svar else { return };
    for u in cfg.svars[var].uses.clone() {
        mark_one_arg_as_this(cfg, u);
    }
}

fn mark_one_arg_as_this(cfg: &mut MethodCfg, arg: ArgRef) {
    {
        let r = cfg.reg_mut(arg);
        r.is_this = true;
        r.immutable_type = true;
    }
    let Some(parent) = arg.parent_insn() else {
        return;
    };
    if cfg.insn(parent).ty == InsnType::Move && arg == ArgRef::Arg(parent, 0) {
        let Some(res) = cfg.insn(parent).result.as_ref() else {
            return;
        };
        let res_reg = res.reg_num;
        let res_var = res.svar;
        let arg_reg = cfg.reg(arg).reg_num;
        if res_reg != arg_reg && !res_var.is_some_and(|v| cfg.svars[v].is_used_in_phi()) {
            mark_this_arg(cfg, ArgRef::Result(parent));
            cfg.insns[parent].flags |= iflag::DONT_GENERATE;
        }
    }
}

fn hide_phi_insns(cfg: &mut MethodCfg) {
    for bid in cfg.blocks.clone() {
        let ids: Vec<InsnId> = cfg.arena[bid]
            .insns
            .iter()
            .copied()
            .filter(|i| cfg.insns[*i].ty != InsnType::Phi)
            .collect();
        cfg.arena[bid].insns = ids;
    }
}

fn remove_unused_invoke_results(cfg: &mut MethodCfg) {
    let mut keep = Vec::with_capacity(cfg.svar_list.len());
    for v in cfg.svar_list.clone() {
        if cfg.svars[v].use_count() == 0 {
            if let Some(p) = cfg.svars[v].assign.parent_insn() {
                if cfg.insn(p).ty == InsnType::Invoke {
                    cfg.insns[p].result = None;
                    continue;
                }
            }
        }
        keep.push(v);
    }
    cfg.svar_list = keep;
}
