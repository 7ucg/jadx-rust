//! `BlockSplitter`: split the instruction array into basic blocks and connect them.

use std::collections::HashMap;

use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::{iflag, BlockId, InsnKind, InsnNode, InsnType};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SplitError(pub String);

fn is_separate(t: InsnType) -> bool {
    matches!(
        t,
        InsnType::Return
            | InsnType::If
            | InsnType::Switch
            | InsnType::MonitorEnter
            | InsnType::MonitorExit
            | InsnType::Throw
            | InsnType::MoveException
    )
}

fn split_without_connect(t: InsnType) -> bool {
    matches!(
        t,
        InsnType::Return
            | InsnType::Throw
            | InsnType::Goto
            | InsnType::If
            | InsnType::Switch
            | InsnType::JavaJsr
            | InsnType::JavaRet
    )
}

fn is_split_by_jump(prev: &InsnNode, cur: &InsnNode) -> bool {
    prev.jumps.iter().any(|j| j.src == prev.offset)
        || cur.jumps.iter().any(|j| j.dest == cur.offset)
}

fn get_block(blocks_map: &HashMap<i32, BlockId>, offset: i32) -> Result<BlockId, SplitError> {
    blocks_map
        .get(&offset)
        .copied()
        .ok_or_else(|| SplitError(format!("Missing block: {offset}")))
}

/// Build the block graph for the instructions (consumes the instruction array contents).
pub fn split_blocks(insns: Vec<Option<InsnNode>>, cfg: &mut MethodCfg) -> Result<(), SplitError> {
    let enter = cfg.start_new_block(-1);
    cfg.block_mut(enter).add(bflag::MTH_ENTER_BLOCK);
    cfg.enter = enter;
    let exit = cfg.start_new_block(-1);
    cfg.block_mut(exit).add(bflag::MTH_EXIT_BLOCK);
    cfg.exit = exit;

    let mut blocks_map: HashMap<i32, BlockId> = HashMap::new();
    let mut cur_block = enter;
    let mut prev: Option<(InsnType, bool, bool, i32, Vec<crate::model::insn::JumpInfo>)> = None;
    let mut prev_node_for_jump: Option<InsnNode> = None;

    for insn in insns.into_iter().flatten() {
        if insn.ty == InsnType::Nop && insn.attrs_empty() {
            continue;
        }
        let off = insn.offset;
        match &prev {
            None => {
                let nb = cfg.start_new_block(off);
                cfg.connect(cur_block, nb);
                cur_block = nb;
            }
            Some((prev_type, _, prev_try_leave, _, _)) => {
                let prev_insn = prev_node_for_jump.as_ref().expect("prev insn");
                if split_without_connect(*prev_type) {
                    cur_block = cfg.start_new_block(off);
                } else if is_separate(*prev_type)
                    || is_separate(insn.ty)
                    || insn.contains_flag(iflag::TRY_ENTER)
                    || *prev_try_leave
                    || insn.exc_handler.is_some()
                    || is_split_by_jump(prev_insn, &insn)
                    || is_do_while(cfg, &blocks_map, cur_block, &insn)
                {
                    let nb = cfg.start_new_block(off);
                    cfg.connect(cur_block, nb);
                    cur_block = nb;
                }
            }
        }
        blocks_map.insert(off, cur_block);
        prev = Some((
            insn.ty,
            false,
            insn.contains_flag(iflag::TRY_LEAVE),
            off,
            Vec::new(),
        ));
        // keep a light copy of the previous instruction for jump checks
        let mut light = InsnNode::plain(insn.ty);
        light.offset = insn.offset;
        light.jumps = insn.jumps.clone();
        prev_node_for_jump = Some(light);
        cfg.block_mut(cur_block).insns.push(insn);
    }
    let _ = prev;

    setup_connections_from_jumps(cfg, &blocks_map)?;
    init_blocks_in_target_nodes(cfg)?;
    remove_jump_attrs(cfg);
    remove_insns(cfg);
    remove_empty_detached_blocks(cfg);
    let order = cfg.blocks.clone();
    let mut kept = Vec::with_capacity(order.len());
    for id in order {
        if !remove_empty_block(cfg, id) {
            kept.push(id);
        } else {
            // removed blocks leave the live list
        }
    }
    cfg.blocks = kept;
    add_temp_connections_for_exc_handlers(cfg, &blocks_map)?;
    setup_exit_connections(cfg);
    cfg.update_block_positions();
    Ok(())
}

fn is_do_while(
    cfg: &MethodCfg,
    blocks_map: &HashMap<i32, BlockId>,
    cur: BlockId,
    insn: &InsnNode,
) -> bool {
    let _ = cfg;
    if insn.ty != InsnType::If {
        return false;
    }
    let InsnKind::If { target, .. } = &insn.kind else {
        return false;
    };
    blocks_map.get(target).copied() == Some(cur)
}

fn setup_connections_from_jumps(
    cfg: &mut MethodCfg,
    blocks_map: &HashMap<i32, BlockId>,
) -> Result<(), SplitError> {
    let order = cfg.blocks.clone();
    for id in order {
        let jumps: Vec<_> = cfg
            .block(id)
            .insns
            .iter()
            .flat_map(|i| i.jumps.clone())
            .collect();
        for j in jumps {
            let src = get_block(blocks_map, j.src)?;
            let dest = get_block(blocks_map, j.dest)?;
            cfg.connect(src, dest);
        }
    }
    Ok(())
}

fn init_blocks_in_target_nodes(cfg: &mut MethodCfg) -> Result<(), SplitError> {
    let order = cfg.blocks.clone();
    for id in order {
        let succs = cfg.block(id).succs.clone();
        let starts: Vec<i32> = succs.iter().map(|s| cfg.block(*s).start_offset).collect();
        let Some(last) = cfg.block_mut(id).insns.last_mut() else {
            continue;
        };
        match &mut last.kind {
            InsnKind::If {
                target,
                then_block,
                else_block,
                ..
            } => {
                let then_idx = starts
                    .iter()
                    .position(|s| s == target)
                    .ok_or_else(|| SplitError(format!("Can't find block by offset: {target}")))?;
                let then = succs[then_idx];
                *then_block = Some(then);
                *else_block = Some(if succs.len() == 1 {
                    then
                } else {
                    select_other(then, &succs)?
                });
            }
            InsnKind::Switch {
                data,
                def,
                target_blocks,
                def_block,
                ..
            } => {
                let Some(d) = data else {
                    return Err(SplitError("Switch data not yet attached".into()));
                };
                let mut tb = Vec::with_capacity(d.targets.len());
                for t in &d.targets {
                    let idx = starts
                        .iter()
                        .position(|s| s == t)
                        .ok_or_else(|| SplitError(format!("Can't find block by offset: {t}")))?;
                    tb.push(succs[idx]);
                }
                *target_blocks = Some(tb);
                let didx = starts
                    .iter()
                    .position(|s| s == def)
                    .ok_or_else(|| SplitError(format!("Can't find block by offset: {def}")))?;
                *def_block = Some(succs[didx]);
            }
            _ => {}
        }
    }
    Ok(())
}

/// `BlockUtils.selectOther` (cleans the list when it has more than two entries is not needed
/// for splitter input: `if` has at most two successors).
fn select_other(node: BlockId, list: &[BlockId]) -> Result<BlockId, SplitError> {
    if list.len() != 2 {
        return Err(SplitError(format!(
            "Incorrect nodes count for selectOther: {node} in {list:?}"
        )));
    }
    Ok(if list[0] != node { list[0] } else { list[1] })
}

fn remove_jump_attrs(cfg: &mut MethodCfg) {
    for id in cfg.blocks.clone() {
        for insn in &mut cfg.block_mut(id).insns {
            insn.jumps.clear();
        }
    }
}

fn remove_insns(cfg: &mut MethodCfg) {
    for id in cfg.blocks.clone() {
        cfg.block_mut(id)
            .insns
            .retain(|i| !(i.attrs_empty() && matches!(i.ty, InsnType::Goto | InsnType::Nop)));
    }
}

pub fn remove_empty_detached_blocks(cfg: &mut MethodCfg) -> bool {
    let before = cfg.blocks.len();
    let keep: Vec<BlockId> = cfg
        .blocks
        .iter()
        .copied()
        .filter(|id| {
            let b = cfg.block(*id);
            !(b.insns.is_empty()
                && b.preds.is_empty()
                && b.succs.is_empty()
                && !b.contains(bflag::MTH_ENTER_BLOCK)
                && !b.contains(bflag::MTH_EXIT_BLOCK))
        })
        .collect();
    cfg.blocks = keep;
    cfg.blocks.len() != before
}

fn can_remove_block(cfg: &MethodCfg, id: BlockId) -> bool {
    let b = cfg.block(id);
    b.insns.is_empty()
        && b.attrs_empty()
        && b.succs.len() <= 1
        && !b.preds.is_empty()
        && !b.contains(bflag::MTH_ENTER_BLOCK)
        && !b.contains(bflag::MTH_EXIT_BLOCK)
        && !b.succs.contains(&id)
}

pub fn replace_target(cfg: &mut MethodCfg, source: BlockId, old: BlockId, new: BlockId) {
    if let Some(last) = cfg.block_mut(source).insns.last_mut() {
        match &mut last.kind {
            InsnKind::If {
                then_block,
                else_block,
                ..
            } => {
                if *then_block == Some(old) {
                    *then_block = Some(new);
                }
                if *else_block == Some(old) {
                    *else_block = Some(new);
                }
            }
            InsnKind::Switch {
                target_blocks,
                def_block,
                ..
            } => {
                if let Some(tb) = target_blocks {
                    for b in tb.iter_mut() {
                        if *b == old {
                            *b = new;
                        }
                    }
                }
                if *def_block == Some(old) {
                    *def_block = Some(new);
                }
            }
            _ => {}
        }
    }
}

pub fn remove_empty_block(cfg: &mut MethodCfg, id: BlockId) -> bool {
    if !can_remove_block(cfg, id) {
        return false;
    }
    let preds = cfg.block(id).preds.clone();
    if cfg.block(id).succs.len() == 1 {
        let successor = cfg.block(id).succs[0];
        for pred in preds {
            cfg.block_mut(pred).succs.retain(|b| *b != id);
            cfg.connect(pred, successor);
            replace_target(cfg, pred, id, successor);
            cfg.update_clean_successors(pred);
        }
        cfg.remove_connection(id, successor);
    } else {
        for pred in preds {
            cfg.block_mut(pred).succs.retain(|b| *b != id);
            cfg.update_clean_successors(pred);
        }
    }
    let b = cfg.block_mut(id);
    b.add(bflag::REMOVE);
    b.succs.clear();
    b.preds.clear();
    true
}

fn add_temp_connections_for_exc_handlers(
    cfg: &mut MethodCfg,
    blocks_map: &HashMap<i32, BlockId>,
) -> Result<(), SplitError> {
    if cfg.handlers.is_empty() {
        return Ok(());
    }
    for id in cfg.blocks.clone() {
        let catches: Vec<Vec<usize>> = cfg
            .block(id)
            .insns
            .iter()
            .filter_map(|i| i.catch.clone())
            .collect();
        for catch in catches {
            for h in catch {
                let off = cfg.handlers[h].handler_offset;
                let handler_block = get_block(blocks_map, off)?;
                if cfg.block(handler_block).tmp_edge.is_none() {
                    let preds = cfg.block(id).preds.clone();
                    if preds.is_empty() {
                        return Err(SplitError(format!(
                            "Unexpected missing predecessor for block: {id}"
                        )));
                    }
                    let start = if preds.len() == 1 { preds[0] } else { id };
                    if !cfg.block(start).succs.contains(&handler_block) {
                        cfg.connect(start, handler_block);
                        cfg.block_mut(handler_block).tmp_edge = Some(start);
                    }
                }
            }
        }
    }
    Ok(())
}

fn setup_exit_connections(cfg: &mut MethodCfg) {
    let exit = cfg.exit;
    for id in cfg.blocks.clone() {
        if cfg.block(id).succs.is_empty() && id != exit {
            cfg.connect(id, exit);
            if cfg.block(id).insns.last().map(|i| i.ty) == Some(InsnType::Return) {
                cfg.block_mut(id).add(bflag::RETURN);
            }
        }
    }
}
