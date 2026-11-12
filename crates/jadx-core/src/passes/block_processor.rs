//! `BlockProcessor`: dominators, loop detection and CFG normalisation.
//!
//! Methods with exception handlers are not supported yet (`BlockExceptionHandler`).

use std::collections::HashMap;

use crate::model::bitset::JavaIntSet;
use crate::model::cfg::{bflag, LoopInfo, MethodCfg};
use crate::model::insn::{BlockId, InsnArg, InsnKind, InsnNode, InsnType};
use crate::passes::block_splitter::{
    remove_empty_block, remove_empty_detached_blocks, replace_target,
};
use crate::passes::dominators::{
    compute_dominance_frontier, compute_dominators, compute_post_dominators, dfs_order,
};

type Res<T> = Result<T, String>;

// ---- connection helpers (`BlockSplitter` statics) ---------------------------------------------

fn replace_connection(cfg: &mut MethodCfg, source: BlockId, old: BlockId, new: BlockId) {
    cfg.remove_connection(source, old);
    cfg.connect(source, new);
    replace_target(cfg, source, old, new);
}

fn insert_block_between(cfg: &mut MethodCfg, source: BlockId, target: BlockId) -> BlockId {
    let off = cfg.block(target).start_offset;
    let nb = cfg.start_new_block(off);
    cfg.block_mut(nb).add(bflag::SYNTHETIC);
    cfg.remove_connection(source, target);
    cfg.connect(source, nb);
    cfg.connect(nb, target);
    replace_target(cfg, source, target, nb);
    cfg.update_clean_successors(source);
    cfg.update_clean_successors(nb);
    nb
}

fn copy_block_data(cfg: &mut MethodCfg, from: BlockId, to: BlockId) {
    let insns = cfg.block(from).insns.clone();
    cfg.block_mut(to).insns.extend(insns);
    copy_block_attributes(cfg, from, to);
}

fn copy_block_attributes(cfg: &mut MethodCfg, from: BlockId, to: BlockId) {
    let f = cfg.block(from).clone();
    let t = cfg.block_mut(to);
    t.flags |= f.flags;
    if f.exc_handler.is_some() {
        t.exc_handler = f.exc_handler;
    }
    if f.tmp_edge.is_some() {
        t.tmp_edge = f.tmp_edge;
    }
    if !f.loops.is_empty() {
        t.loops = f.loops.clone();
    }
}

fn remove_predecessors(cfg: &mut MethodCfg, block: BlockId) {
    for pred in cfg.block(block).preds.clone() {
        cfg.block_mut(pred).succs.retain(|b| *b != block);
    }
    cfg.block_mut(block).preds.clear();
}

pub fn detach_block(cfg: &mut MethodCfg, block: BlockId) {
    for pred in cfg.block(block).preds.clone() {
        cfg.block_mut(pred).succs.retain(|b| *b != block);
        cfg.update_clean_successors(pred);
    }
    for succ in cfg.block(block).succs.clone() {
        cfg.block_mut(succ).preds.retain(|b| *b != block);
    }
    let b = cfg.block_mut(block);
    b.add(bflag::REMOVE);
    b.preds.clear();
    b.succs.clear();
}

// ---- main driver ----------------------------------------------------------------------------

pub fn process_blocks(cfg: &mut MethodCfg) -> Res<()> {
    if cfg.blocks.is_empty() {
        return Ok(());
    }
    if !cfg.handlers.is_empty() {
        return Err("exception handler processing is not supported yet".into());
    }
    remove_unreachable_blocks(cfg);
    compute_dominators_and_loops(cfg)?;
    if independent_block_tree_mod(cfg) {
        check_for_unreachable_blocks(cfg)?;
        compute_dominators_and_loops(cfg)?;
    }
    if fix_multi_entry_loops(cfg)? {
        compute_dominators_and_loops(cfg)?;
    }
    update_clean_successors_all(cfg);
    let blocks_count = cfg.blocks.len();
    let mod_limit = blocks_count.max(100);
    let mut i = 0;
    while modify_blocks_tree(cfg) {
        compute_dominators_and_loops(cfg)?;
        i += 1;
        if i > mod_limit {
            break;
        }
    }
    check_for_unreachable_blocks(cfg)?;
    compute_dominance_frontier(cfg);
    register_loops(cfg);
    process_nested_loops(cfg);
    compute_post_dominators(cfg);
    update_clean_successors_all(cfg);
    Ok(())
}

fn update_clean_successors_all(cfg: &mut MethodCfg) {
    for b in cfg.blocks.clone() {
        cfg.update_clean_successors(b);
    }
}

fn compute_dominators_and_loops(cfg: &mut MethodCfg) -> Res<()> {
    clear_blocks_state(cfg);
    compute_dominators(cfg).map_err(|e| e.0)?;
    mark_loops(cfg);
    Ok(())
}

fn clear_blocks_state(cfg: &mut MethodCfg) {
    for b in cfg.blocks.clone() {
        let blk = cfg.block_mut(b);
        blk.loops.clear();
        blk.flags &= !(bflag::LOOP_START | bflag::LOOP_END);
        blk.doms = None;
        blk.idom = None;
        blk.dom_frontier = None;
        blk.dominates_on.clear();
    }
}

/// `BlockUtils.getAllPathsBlocks`
fn all_paths_blocks(cfg: &MethodCfg, start: BlockId, end: BlockId) -> JavaIntSet {
    let mut set = JavaIntSet::new();
    set.insert(start);
    if start != end {
        add_predecessors(cfg, &mut set, end, start);
    }
    set
}

fn add_predecessors(cfg: &MethodCfg, set: &mut JavaIntSet, from: BlockId, until: BlockId) {
    set.insert(from);
    for &pred in &cfg.block(from).preds {
        if pred != until && !set.contains(pred) {
            add_predecessors(cfg, set, pred, until);
        }
    }
}

fn mark_loops(cfg: &mut MethodCfg) {
    for block in cfg.blocks.clone() {
        for succ in cfg.block(block).succs.clone() {
            let dominated = cfg
                .block(block)
                .doms
                .as_ref()
                .is_some_and(|d| d.get(cfg.block(succ).pos));
            if dominated || block == succ {
                cfg.block_mut(succ).add(bflag::LOOP_START);
                cfg.block_mut(block).add(bflag::LOOP_END);
                let blocks = all_paths_blocks(cfg, succ, block);
                let id = cfg.loop_infos.len();
                cfg.loop_infos.push(LoopInfo {
                    start: succ,
                    end: block,
                    blocks,
                    id: 0,
                    parent: None,
                });
                cfg.block_mut(succ).loops.push(id);
                cfg.block_mut(block).loops.push(id);
            }
        }
    }
}

fn register_loops(cfg: &mut MethodCfg) {
    cfg.loops.clear();
    for block in cfg.blocks.clone() {
        if cfg.block(block).contains(bflag::LOOP_START) {
            for l in cfg.block(block).loops.clone() {
                cfg.loop_infos[l].id = cfg.loops.len();
                cfg.loops.push(l);
            }
        }
    }
}

fn process_nested_loops(cfg: &mut MethodCfg) {
    if cfg.loops.is_empty() {
        return;
    }
    let loops = cfg.loops.clone();
    for &out in &loops {
        for &inner in &loops {
            if out == inner {
                continue;
            }
            let contains = cfg.loop_infos[out]
                .blocks
                .contains_all(&cfg.loop_infos[inner].blocks);
            if contains {
                match cfg.loop_infos[inner].parent {
                    Some(parent) => {
                        if cfg.loop_infos[parent]
                            .blocks
                            .contains_all(&cfg.loop_infos[out].blocks)
                        {
                            cfg.loop_infos[out].parent = Some(parent);
                            cfg.loop_infos[inner].parent = Some(out);
                        } else {
                            cfg.loop_infos[parent].parent = Some(out);
                        }
                    }
                    None => cfg.loop_infos[inner].parent = Some(out),
                }
            }
        }
    }
}

// ---- unreachable blocks ---------------------------------------------------------------------

fn collect_successors(
    cfg: &MethodCfg,
    start: BlockId,
    enter: BlockId,
    to_remove: &mut Vec<BlockId>,
) {
    let mut stack = vec![start];
    while let Some(block) = stack.pop() {
        if !to_remove.contains(&block) {
            to_remove.push(block);
            for &s in &cfg.block(block).succs {
                if s != enter && cfg.block(s).preds.iter().all(|p| to_remove.contains(p)) {
                    stack.push(s);
                }
            }
        }
    }
}

fn remove_unreachable_blocks(cfg: &mut MethodCfg) {
    let mut to_remove: Vec<BlockId> = Vec::new();
    for block in cfg.blocks.clone() {
        if cfg.block(block).preds.is_empty() && block != cfg.enter {
            collect_successors(cfg, block, cfg.enter, &mut to_remove);
        }
    }
    if to_remove.is_empty() {
        return;
    }
    for &b in &to_remove {
        detach_block(cfg, b);
    }
    cfg.blocks.retain(|b| !to_remove.contains(b));
    cfg.update_block_positions();
}

fn check_for_unreachable_blocks(cfg: &MethodCfg) -> Res<()> {
    for &b in &cfg.blocks {
        if cfg.block(b).preds.is_empty() && b != cfg.enter {
            return Err(format!("Unreachable block: {b}"));
        }
    }
    Ok(())
}

// ---- tree modifications -----------------------------------------------------------------------

fn independent_block_tree_mod(cfg: &mut MethodCfg) -> bool {
    let mut changed = false;
    for b in cfg.blocks.clone() {
        if deduplicate_block_insns(cfg, b) {
            changed = true;
        }
    }
    // BlockExceptionHandler.process: no handlers in supported methods
    for b in cfg.blocks.clone() {
        if remove_empty_block(cfg, b) {
            changed = true;
        }
    }
    if remove_empty_detached_blocks(cfg) {
        changed = true;
    }
    changed
}

fn is_same_arg(a: &InsnArg, b: &InsnArg) -> bool {
    match (a, b) {
        (InsnArg::Register(x), InsnArg::Register(y)) => x.reg_num == y.reg_num,
        (InsnArg::Literal(x), InsnArg::Literal(y)) => x.literal == y.literal,
        _ => false,
    }
}

fn same_result(
    a: &Option<crate::model::insn::RegisterArg>,
    b: &Option<crate::model::insn::RegisterArg>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.reg_num == y.reg_num,
        _ => false,
    }
}

/// `InsnNode.isSame` for the instruction kinds that can occur at this stage.
fn insn_is_same(a: &InsnNode, b: &InsnNode) -> bool {
    if a.ty != b.ty || a.args.len() != b.args.len() {
        return false;
    }
    match (&a.kind, &b.kind) {
        (InsnKind::None, InsnKind::None) => true,
        (InsnKind::Goto { .. }, InsnKind::Goto { .. }) => true,
        (InsnKind::If { op: x, .. }, InsnKind::If { op: y, .. }) => x == y,
        (
            InsnKind::Switch {
                data_target: d1,
                packed: p1,
                ..
            },
            InsnKind::Switch {
                data_target: d2,
                packed: p2,
                ..
            },
        ) => d1 == d2 && p1 == p2,
        (InsnKind::ConstStr(x), InsnKind::ConstStr(y)) => x == y,
        (InsnKind::ConstClass(x), InsnKind::ConstClass(y)) => x == y,
        (InsnKind::Index(x), InsnKind::Index(y)) => x == y,
        (
            InsnKind::Invoke {
                invoke_type: t1,
                mth: m1,
            },
            InsnKind::Invoke {
                invoke_type: t2,
                mth: m2,
            },
        ) => t1 == t2 && m1 == m2,
        (InsnKind::Arith(x), InsnKind::Arith(y)) => {
            if x != y {
                return false;
            }
            match (a.args.get(1), b.args.get(1)) {
                (Some(InsnArg::Literal(l1)), Some(InsnArg::Literal(l2))) => {
                    l1.literal == l2.literal
                }
                (Some(InsnArg::Literal(_)), _) | (_, Some(InsnArg::Literal(_))) => false,
                _ => true,
            }
        }
        // identity comparisons in the reference: effectively never equal
        (InsnKind::NewArray(_), InsnKind::NewArray(_)) => false,
        (InsnKind::FilledNewArray(x), InsnKind::FilledNewArray(y)) => x.is_primitive() && x == y,
        (InsnKind::FillArray { .. }, InsnKind::FillArray { .. }) => false,
        (InsnKind::FillArrayData(_), InsnKind::FillArrayData(_)) => false,
        _ => false,
    }
}

fn can_reorder(i: &InsnNode) -> bool {
    use crate::model::insn::iflag;
    if i.contains_flag(iflag::DONT_GENERATE) {
        return i.ty != InsnType::MonitorExit;
    }
    matches!(
        i.ty,
        InsnType::Const
            | InsnType::ConstStr
            | InsnType::ConstClass
            | InsnType::Cast
            | InsnType::Move
            | InsnType::Arith
            | InsnType::Neg
            | InsnType::CmpL
            | InsnType::CmpG
            | InsnType::CheckCast
            | InsnType::InstanceOf
            | InsnType::FillArray
            | InsnType::FilledNewArray
            | InsnType::NewArray
            | InsnType::StrConcat
    )
}

fn insns_equal(a: &InsnNode, b: &InsnNode) -> bool {
    if insn_is_same(a, b) && same_result(&a.result, &b.result) {
        return a.args.iter().zip(&b.args).all(|(x, y)| is_same_arg(x, y));
    }
    false
}

fn insn_from_end(cfg: &MethodCfg, block: BlockId, number: usize) -> Option<&InsnNode> {
    let insns = &cfg.block(block).insns;
    if insns.len() <= number {
        return None;
    }
    Some(&insns[insns.len() - number - 1])
}

fn same_last_insn_count(cfg: &MethodCfg, preds: &[BlockId]) -> usize {
    let mut count = 0;
    loop {
        let mut first: Option<&InsnNode> = None;
        for &p in preds {
            let Some(cur) = insn_from_end(cfg, p, count) else {
                return count;
            };
            match first {
                None => first = Some(cur),
                Some(f) => {
                    if !(insns_equal(f, cur) && can_reorder(f)) {
                        return count;
                    }
                }
            }
        }
        count += 1;
    }
}

fn deduplicate_block_insns(cfg: &mut MethodCfg, block: BlockId) -> bool {
    let b = cfg.block(block);
    if !(b.contains(bflag::LOOP_START) || b.contains(bflag::LOOP_END)) {
        return false;
    }
    let preds = b.preds.clone();
    if preds.len() <= 1 {
        return false;
    }
    if b.insns.last().map(|i| i.ty) == Some(InsnType::If) {
        return false;
    }
    if b.insns.first().is_some_and(|i| i.exc_handler.is_some()) {
        return false;
    }
    let same = same_last_insn_count(cfg, &preds);
    if same == 0 {
        return false;
    }
    let first_pred = &cfg.block(preds[0]).insns;
    let moved: Vec<InsnNode> = first_pred[first_pred.len() - same..].to_vec();
    let blk = cfg.block_mut(block);
    let mut new_list = moved;
    new_list.append(&mut blk.insns);
    blk.insns = new_list;
    for p in preds {
        let ins = &mut cfg.block_mut(p).insns;
        let len = ins.len();
        ins.truncate(len - same);
    }
    true
}

fn modify_blocks_tree(cfg: &mut MethodCfg) -> bool {
    for block in cfg.blocks.clone() {
        if check_loops(cfg, block) {
            return true;
        }
    }
    if merge_const_return(cfg) {
        return true;
    }
    if cfg.has_switch {
        for block in cfg.blocks.clone() {
            if duplicate_simple_move_block(cfg, block) {
                return true;
            }
        }
    }
    split_exit_blocks(cfg)
}

fn merge_const_return(cfg: &mut MethodCfg) -> bool {
    if cfg.void_return {
        return false;
    }
    let mut changed = false;
    for ret_block in cfg.block(cfg.exit).preds.clone() {
        let rb = cfg.block(ret_block);
        if rb.preds.len() != 1 {
            continue;
        }
        let pred = rb.preds[0];
        let pb = cfg.block(pred);
        if pb.insns.len() != 1 {
            continue;
        }
        let const_insn = &pb.insns[0];
        if !matches!(
            const_insn.ty,
            InsnType::Const | InsnType::ConstStr | InsnType::ConstClass
        ) {
            continue;
        }
        let Some(const_arg) = &const_insn.result else {
            continue;
        };
        let Some(ret_insn) = rb.insns.last() else {
            continue;
        };
        if ret_insn.ty != InsnType::Return {
            continue;
        }
        let Some(InsnArg::Register(ret_reg)) = ret_insn.args.first() else {
            continue;
        };
        if const_arg.reg_num == ret_reg.reg_num {
            merge_const_and_return_blocks(cfg, ret_block, pred);
            changed = true;
        }
    }
    if changed {
        remove_marked_blocks(cfg);
    }
    changed
}

fn merge_const_and_return_blocks(cfg: &mut MethodCfg, ret_block: BlockId, pred: BlockId) {
    let ret_insns = cfg.block(ret_block).insns.clone();
    cfg.block_mut(pred).insns.extend(ret_insns);
    copy_block_attributes(cfg, ret_block, pred);
    cfg.remove_connection(pred, ret_block);
    cfg.block_mut(ret_block).insns.clear();
    cfg.block_mut(ret_block).add(bflag::REMOVE);
    let exit = cfg.exit;
    cfg.remove_connection(ret_block, exit);
    cfg.connect(pred, exit);
    cfg.update_clean_successors(pred);
}

pub fn remove_marked_blocks(cfg: &mut MethodCfg) {
    let mut removed = false;
    let blocks = cfg.blocks.clone();
    let mut kept = Vec::with_capacity(blocks.len());
    for b in blocks {
        let blk = cfg.block(b);
        if blk.contains(bflag::REMOVE) && blk.preds.is_empty() && blk.succs.is_empty() {
            removed = true;
        } else {
            kept.push(b);
        }
    }
    cfg.blocks = kept;
    if removed {
        cfg.update_block_positions();
    }
}

fn only_switch_and_if_in_last_insns(cfg: &MethodCfg, preds: &[BlockId]) -> bool {
    let (mut has_switch, mut has_if) = (false, false);
    for &p in preds {
        match cfg.block(p).insns.last().map(|i| i.ty) {
            Some(InsnType::Switch) => has_switch = true,
            Some(InsnType::If) => has_if = true,
            _ => return false,
        }
    }
    has_switch && has_if
}

fn duplicate_simple_move_block(cfg: &mut MethodCfg, block: BlockId) -> bool {
    let b = cfg.block(block);
    if b.insns.len() != 1 || b.succs.len() != 1 || b.insns[0].ty != InsnType::Move {
        return false;
    }
    let preds = b.preds.clone();
    if preds.len() < 3 || !only_switch_and_if_in_last_insns(cfg, &preds) {
        return false;
    }
    let successor = b.succs[0];
    for pred in preds.iter().skip(1) {
        let nb = cfg.start_new_block(-1);
        cfg.block_mut(nb).add(bflag::SYNTHETIC);
        for old in cfg.block(block).insns.clone() {
            let mut c = old;
            c.flags |= crate::model::insn::iflag::SYNTHETIC;
            cfg.block_mut(nb).insns.push(c);
        }
        copy_block_attributes(cfg, block, nb);
        replace_connection(cfg, *pred, block, nb);
        cfg.connect(nb, successor);
    }
    true
}

fn check_loops(cfg: &mut MethodCfg, block: BlockId) -> bool {
    if !cfg.block(block).contains(bflag::LOOP_START) {
        return false;
    }
    let loops = cfg.block(block).loops.clone();
    if loops.is_empty() {
        return false;
    }
    for &l in &loops {
        if insert_blocks_for_break(cfg, l) {
            return true;
        }
    }
    if loops.len() > 1 && split_loops(cfg, block, &loops) {
        return true;
    }
    if loops.len() == 1 {
        let l = loops[0];
        return insert_blocks_for_continue(cfg, l)
            || insert_pre_header(cfg, l)
            || simplify_loop_end(cfg, l);
    }
    false
}

fn simplify_loop_end(cfg: &mut MethodCfg, l: usize) -> bool {
    let (start, end) = (cfg.loop_infos[l].start, cfg.loop_infos[l].end);
    if cfg.block(end).succs.len() <= 1 {
        return false;
    }
    let nb = cfg.start_new_block(-1);
    cfg.block_mut(nb).add(bflag::SYNTHETIC);
    cfg.block_mut(nb).add(bflag::LOOP_END);
    replace_connection(cfg, end, start, nb);
    cfg.connect(nb, start);
    true
}

fn insert_pre_header(cfg: &mut MethodCfg, l: usize) -> bool {
    let (start, end) = (cfg.loop_infos[l].start, cfg.loop_infos[l].end);
    let preds = cfg.block(start).preds.clone();
    let preds_count = preds.len() as isize - 1; // don't count back edge
    if preds_count == 1 {
        return false;
    }
    if preds_count == 0 {
        let nb = cfg.start_new_block(-1);
        cfg.block_mut(nb).add(bflag::SYNTHETIC);
        cfg.block_mut(nb).add(bflag::MTH_ENTER_BLOCK);
        cfg.enter = nb;
        cfg.block_mut(start).flags &= !bflag::MTH_ENTER_BLOCK;
        cfg.connect(nb, start);
    } else {
        let pre = cfg.start_new_block(-1);
        cfg.block_mut(pre).add(bflag::SYNTHETIC);
        for p in preds {
            if p != end {
                replace_connection(cfg, p, start, pre);
            }
        }
        cfg.connect(pre, start);
    }
    true
}

fn insert_blocks_for_break(cfg: &mut MethodCfg, l: usize) -> bool {
    let mut change = false;
    let blocks = cfg.loop_infos[l].blocks.iter();
    let mut edges: Vec<(BlockId, BlockId)> = Vec::new();
    for b in blocks {
        for &s in &cfg.block(b).succs {
            if !cfg.loop_infos[l].blocks.contains(s) && !cfg.is_exception_handler_path(s) {
                edges.push((b, s));
            }
        }
    }
    for (source, target) in edges {
        if !cfg.block(target).contains(bflag::SYNTHETIC)
            && !cfg.block(source).contains(bflag::SYNTHETIC)
        {
            insert_block_between(cfg, source, target);
            change = true;
        }
    }
    change
}

fn insert_blocks_for_continue(cfg: &mut MethodCfg, l: usize) -> bool {
    let end = cfg.loop_infos[l].end;
    let mut change = false;
    let preds = cfg.block(end).preds.clone();
    if preds.len() > 1 {
        for pred in preds {
            if !cfg.block(pred).contains(bflag::SYNTHETIC) {
                insert_block_between(cfg, pred, end);
                change = true;
            }
        }
    }
    change
}

fn split_loops(cfg: &mut MethodCfg, block: BlockId, loops: &[usize]) -> bool {
    if !loops.iter().all(|l| cfg.loop_infos[*l].start == block) {
        return false;
    }
    let off = cfg.block(block).start_offset;
    let nb = cfg.start_new_block(off);
    cfg.block_mut(nb).add(bflag::SYNTHETIC);
    cfg.connect(nb, block);
    for &l in loops {
        let end = cfg.loop_infos[l].end;
        replace_connection(cfg, end, block, nb);
    }
    true
}

fn split_exit_blocks(cfg: &mut MethodCfg) -> bool {
    let mut changed = false;
    for pre_exit in cfg.block(cfg.exit).preds.clone() {
        if split_return(cfg, pre_exit) || split_throw(cfg, pre_exit) {
            changed = true;
        }
    }
    if changed {
        update_exit_block_connections(cfg);
    }
    changed
}

fn update_exit_block_connections(cfg: &mut MethodCfg) {
    let exit = cfg.exit;
    remove_predecessors(cfg, exit);
    for b in cfg.blocks.clone() {
        if b != exit && cfg.block(b).succs.is_empty() && !cfg.block(b).contains(bflag::REMOVE) {
            cfg.connect(b, exit);
        }
    }
}

fn is_arg_assign_in_pred(cfg: &MethodCfg, preds: &[BlockId], arg: &InsnArg) -> bool {
    if let InsnArg::Register(r) = arg {
        for &p in preds {
            for i in &cfg.block(p).insns {
                if i.result
                    .as_ref()
                    .is_some_and(|res| res.reg_num == r.reg_num)
                {
                    return true;
                }
            }
        }
    }
    false
}

fn split_return(cfg: &mut MethodCfg, return_block: BlockId) -> bool {
    let rb = cfg.block(return_block);
    if rb.contains(bflag::SYNTHETIC) || rb.contains(bflag::ORIG_RETURN) || rb.exc_handler.is_some()
    {
        return false;
    }
    let preds = rb.preds.clone();
    if preds.len() < 2 {
        return false;
    }
    let Some(return_insn) = rb.insns.last() else {
        return false;
    };
    if return_insn.args.len() == 1
        && rb.insns.len() == 1
        && !is_arg_assign_in_pred(cfg, &preds, &return_insn.args[0])
    {
        return false;
    }
    let mut first = true;
    for pred in preds {
        if first {
            cfg.block_mut(return_block).add(bflag::ORIG_RETURN);
            first = false;
        } else {
            let nb = cfg.start_new_block(-1);
            cfg.block_mut(nb).add(bflag::SYNTHETIC);
            cfg.block_mut(nb).add(bflag::RETURN);
            for old in cfg.block(return_block).insns.clone() {
                let mut c = old;
                c.flags |= crate::model::insn::iflag::SYNTHETIC;
                cfg.block_mut(nb).insns.push(c);
            }
            replace_connection(cfg, pred, return_block, nb);
        }
    }
    true
}

fn split_throw(cfg: &mut MethodCfg, exit_block: BlockId) -> bool {
    if cfg.block(exit_block).contains(bflag::IGNORE_THROW_SPLIT) {
        return false;
    }
    let preds = cfg.block(exit_block).preds.clone();
    if preds.len() < 2 {
        return false;
    }
    let Some(throw_insn) = cfg.block(exit_block).insns.last() else {
        return false;
    };
    if throw_insn.ty != InsnType::Throw {
        return false;
    }
    // handler discovery: no exception handlers in supported methods, so no handler blocks found
    let handlers_found: HashMap<BlockId, ()> = HashMap::new();
    if handlers_found.len() == 1 {
        cfg.block_mut(exit_block).add(bflag::IGNORE_THROW_SPLIT);
        return false;
    }
    let mut first = true;
    for pred in preds {
        if first {
            first = false;
        } else {
            let nb = cfg.start_new_block(-1);
            cfg.block_mut(nb).add(bflag::SYNTHETIC);
            for old in cfg.block(exit_block).insns.clone() {
                let mut c = old;
                c.flags |= crate::model::insn::iflag::SYNTHETIC;
                cfg.block_mut(nb).insns.push(c);
            }
            copy_block_attributes(cfg, exit_block, nb);
            replace_connection(cfg, pred, exit_block, nb);
        }
    }
    true
}

// ---- multi entry loops ------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Color {
    White,
    Gray,
    Black,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EdgeType {
    Back,
    Cross,
}

struct SpecialEdge {
    ty: EdgeType,
    start: BlockId,
    end: BlockId,
}

fn color_dfs(cfg: &MethodCfg, colors: &mut [Color], block: BlockId, out: &mut Vec<SpecialEdge>) {
    colors[cfg.block(block).pos] = Color::Gray;
    for &v in &cfg.block(block).succs {
        match colors[cfg.block(v).pos] {
            Color::White => color_dfs(cfg, colors, v, out),
            Color::Gray => out.push(SpecialEdge {
                ty: EdgeType::Back,
                start: block,
                end: v,
            }),
            Color::Black => out.push(SpecialEdge {
                ty: EdgeType::Cross,
                start: block,
                end: v,
            }),
        }
    }
    colors[cfg.block(block).pos] = Color::Black;
}

fn fix_multi_entry_loops(cfg: &mut MethodCfg) -> Res<bool> {
    let mut colors = vec![Color::White; cfg.blocks.len()];
    let mut edges = Vec::new();
    color_dfs(cfg, &mut colors, cfg.enter, &mut edges);
    let multi: Vec<&SpecialEdge> = edges
        .iter()
        .filter(|e| e.ty == EdgeType::Back && !is_single_entry_loop(cfg, e))
        .collect();
    if multi.is_empty() {
        return Ok(false);
    }
    let cross: Vec<&SpecialEdge> = edges.iter().filter(|e| e.ty == EdgeType::Cross).collect();
    let mut changed = false;
    for back in multi {
        changed |= fix_loop(cfg, back, &cross);
    }
    Ok(changed)
}

fn is_single_entry_loop(cfg: &MethodCfg, e: &SpecialEdge) -> bool {
    let (header, loop_end) = (e.end, e.start);
    header == loop_end
        || cfg
            .block(loop_end)
            .doms
            .as_ref()
            .is_some_and(|d| d.get(cfg.block(header).pos))
}

fn filter_only_one<'a>(
    list: &[&'a SpecialEdge],
    pred: impl Fn(&SpecialEdge) -> bool,
) -> Option<&'a SpecialEdge> {
    let mut it = list.iter().filter(|e| pred(e));
    let first = it.next()?;
    if it.next().is_some() {
        return None;
    }
    Some(first)
}

fn fix_loop(cfg: &mut MethodCfg, back: &SpecialEdge, cross: &[&SpecialEdge]) -> bool {
    // header successor entry
    let header = back.end;
    let header_idom = cfg.block(header).idom;
    if let Some(sub) = filter_only_one(cross, |e| Some(e.start) == header_idom) {
        if cfg.block(header).succs.len() == 1 && cfg.block(header).succs[0] == sub.end {
            let (loop_end, sub_entry) = (back.start, sub.end);
            let copy_header = insert_block_between(cfg, loop_end, header);
            copy_block_data(cfg, header, copy_header);
            replace_connection(cfg, copy_header, header, sub_entry);
            return true;
        }
    }
    // end block entry
    let loop_end = back.start;
    if let Some(sub) = filter_only_one(cross, |e| e.end == loop_end) {
        let copy_center = insert_block_between(cfg, sub.start, back.end);
        copy_block_data(cfg, loop_end, copy_center);
        cfg.remove_connection(sub.start, loop_end);
        return true;
    }
    false
}

/// For tests / debugging: DFS order of live blocks.
pub fn live_dfs(cfg: &MethodCfg) -> Vec<BlockId> {
    dfs_order(cfg, cfg.enter, false)
}
