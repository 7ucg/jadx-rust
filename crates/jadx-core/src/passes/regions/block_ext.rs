//! `BlockUtils` helpers needed by the region stages.

use std::collections::HashSet;

use crate::model::bitset::BitSet;
use crate::model::cfg::{bflag, MethodCfg};
use crate::model::insn::{BlockId, InsnArg, InsnId, InsnKind, InsnNode, InsnType};
use crate::passes::block_utils::{
    follow_empty_path, is_any_path_exists, is_dominator, is_path_exists,
};
use crate::passes::regions::utils::{cs, next_block};

/// `BlockUtils.bitSetToBlocks`
pub fn bit_set_to_blocks(cfg: &MethodCfg, bs: &BitSet) -> Vec<BlockId> {
    bs.iter()
        .filter_map(|i| cfg.blocks.get(i).copied())
        .collect()
}

/// `BlockUtils.bitSetToOneBlock`
pub fn bit_set_to_one_block(cfg: &MethodCfg, bs: &BitSet) -> Option<BlockId> {
    if bs.cardinality() != 1 {
        return None;
    }
    cfg.blocks.get(bs.next_set_bit(0)?).copied()
}

/// `BlockUtils.blocksToBitSet`
pub fn blocks_to_bit_set<'a>(
    cfg: &MethodCfg,
    blocks: impl IntoIterator<Item = &'a BlockId>,
) -> BitSet {
    let mut bs = BitSet::new();
    for b in blocks {
        bs.set(cfg.block(*b).pos);
    }
    bs
}

/// Dominance frontier of `b` as a bit set (empty when not computed).
pub fn dom_frontier(cfg: &MethodCfg, b: BlockId) -> BitSet {
    cfg.block(b).dom_frontier.clone().unwrap_or_default()
}

pub fn pos(cfg: &MethodCfg, b: BlockId) -> usize {
    cfg.block(b).pos
}

/// `BlockUtils.selectOther`
pub fn select_other(cfg: &MethodCfg, node: BlockId, blocks: &[BlockId]) -> Result<BlockId, String> {
    let list: Vec<BlockId> = if blocks.len() > 2 {
        blocks
            .iter()
            .copied()
            .filter(|b| !cfg.is_exception_handler_path(*b))
            .collect()
    } else {
        blocks.to_vec()
    };
    if list.len() != 2 {
        return Err(format!(
            "Incorrect nodes count for selectOther: {node} in {list:?}"
        ));
    }
    Ok(if list[0] != node { list[0] } else { list[1] })
}

/// `BlockUtils.isFollowBackEdge`
pub fn is_follow_back_edge(cfg: &MethodCfg, block: Option<BlockId>) -> bool {
    let Some(block) = block else {
        return false;
    };
    if cfg.block(block).contains(bflag::LOOP_START) {
        let preds = &cfg.block(block).preds;
        if preds.len() == 1 {
            let end = preds[0];
            if cfg.block(end).contains(bflag::LOOP_END) {
                for l in &cfg.block(end).loops {
                    let li = &cfg.loop_infos[*l];
                    if li.start == block && li.end == end {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// `BlockUtils.followEmptyPath(start)`
pub fn follow_empty(cfg: &MethodCfg, start: BlockId) -> BlockId {
    follow_empty_path(cfg, start, false, true)
}

/// `BlockUtils.followEmptyPath(start, reverse)`
pub fn follow_empty_rev(cfg: &MethodCfg, start: BlockId, reverse: bool) -> BlockId {
    follow_empty_path(cfg, start, reverse, true)
}

fn next_on_empty_path(cfg: &MethodCfg, block: BlockId, reverse: bool) -> Option<BlockId> {
    let b = cfg.block(block);
    if !b.insns.is_empty()
        || (!reverse && b.preds.len() > 1)
        || (reverse && cs(cfg, block).len() > 1)
    {
        return None;
    }
    let next = if reverse {
        b.preds.clone()
    } else {
        cs(cfg, block)
    };
    if next.len() != 1 {
        return None;
    }
    Some(next[0])
}

/// `BlockUtils.visitBlocksOnEmptyPath` (clean successors)
pub fn blocks_on_empty_path(cfg: &MethodCfg, start: BlockId) -> Vec<BlockId> {
    let mut v = Vec::new();
    let mut cur = start;
    while let Some(next) = next_on_empty_path(cfg, cur, false) {
        v.push(next);
        cur = next;
    }
    v
}

/// `BlockUtils.isEmptySimplePath`
pub fn is_empty_simple_path(cfg: &MethodCfg, start: BlockId, end: BlockId) -> bool {
    if start == end && cfg.block(start).insns.is_empty() {
        return true;
    }
    if !cfg.block(start).insns.is_empty() || cs(cfg, start).len() != 1 {
        return false;
    }
    let mut block = next_block(cfg, start);
    while let Some(b) = block {
        if b != end
            && cs(cfg, b).len() < 2
            && cfg.block(b).preds.len() == 1
            && cfg.block(b).insns.is_empty()
        {
            block = next_block(cfg, b);
        } else {
            break;
        }
    }
    block == Some(end)
}

/// `BlockUtils.skipSyntheticPredecessor`
pub fn skip_synthetic_predecessor(cfg: &MethodCfg, block: BlockId) -> BlockId {
    if cfg.block(block).contains(bflag::SYNTHETIC)
        && cfg.block(block).insns.is_empty()
        && cfg.block(block).preds.len() == 1
    {
        return cfg.block(block).preds[0];
    }
    block
}

/// `BlockUtils.buildSimplePath`
pub fn build_simple_path(cfg: &MethodCfg, block: BlockId) -> Vec<BlockId> {
    if cs(cfg, block).len() >= 2 {
        return Vec::new();
    }
    let mut list = vec![block];
    let mut current = next_block(cfg, block);
    while let Some(c) = current {
        if cs(cfg, c).len() < 2 && cfg.block(c).preds.len() == 1 {
            list.push(c);
            current = next_block(cfg, c);
        } else {
            break;
        }
    }
    list
}

/// `BlockUtils.getDomFrontierThroughEdge`
pub fn get_dom_frontier_through_edge(cfg: &MethodCfg, target: BlockId) -> BitSet {
    if cfg.block(target).preds.len() > 1 {
        let mut df = BitSet::new();
        df.set(cfg.block(target).pos);
        df
    } else {
        dom_frontier(cfg, target)
    }
}

/// `BlockUtils.getTopSplitterForHandler`
pub fn get_top_splitter_for_handler(
    cfg: &MethodCfg,
    handler_block: BlockId,
) -> Result<BlockId, String> {
    cfg.block(handler_block)
        .preds
        .iter()
        .copied()
        .find(|b| cfg.block(*b).contains(bflag::EXC_TOP_SPLITTER))
        .ok_or_else(|| format!("Can't find top splitter block for handler:{handler_block}"))
}

/// `BlockUtils.getTryAndHandlerCrossBlock`
pub fn get_try_and_handler_cross_block(
    cfg: &MethodCfg,
    handler: usize,
) -> Result<Option<BlockId>, String> {
    let Some(start) = cfg.handlers[handler].handler_block else {
        return Err("null handler block".to_string());
    };
    let top_splitter = get_top_splitter_for_handler(cfg, start)?;
    let try_block = cfg.handlers[handler]
        .try_block
        .ok_or_else(|| "handler without try block".to_string())?;
    let all_handlers: Vec<usize> = cfg.try_blocks[try_block].handlers.clone();
    let mut candidates: Vec<BlockId> = bit_set_to_blocks(cfg, &dom_frontier(cfg, start));
    let mut visited = BitSet::new();
    while !candidates.is_empty() {
        let frontier = candidates.remove(0);
        let fpos = cfg.block(frontier).pos;
        if visited.get(fpos) {
            continue;
        }
        visited.set(fpos);
        for &pred in &cfg.block(frontier).preds {
            let pred_from_handler = all_handlers.iter().any(|h| {
                cfg.handlers[*h]
                    .handler_block
                    .is_some_and(|hb| is_path_exists(cfg, hb, pred))
            });
            if !pred_from_handler && is_path_exists(cfg, top_splitter, pred) && frontier != cfg.exit
            {
                return Ok(Some(frontier));
            }
        }
        candidates.extend(bit_set_to_blocks(cfg, &dom_frontier(cfg, frontier)));
    }
    Ok(None)
}

/// `BlockUtils.getBottomBlock(blocks, clean)`
pub fn get_bottom_block(cfg: &MethodCfg, blocks: &[BlockId], clean: bool) -> Option<BlockId> {
    if blocks.len() == 1 {
        return Some(blocks[0]);
    }
    if !clean {
        for &cand in blocks {
            if blocks
                .iter()
                .all(|&from| cand == from || is_dominator(cfg, cand, from))
            {
                return Some(cand);
            }
        }
    }
    for &cand in blocks {
        let ok = blocks.iter().all(|&from| {
            cand == from
                || if clean {
                    is_path_exists(cfg, from, cand)
                } else {
                    is_any_path_exists(cfg, from, cand)
                }
        });
        if ok {
            return Some(cand);
        }
    }
    None
}

/// `BlockUtils.collectBlocksDominatedBy` (clean successors only)
pub fn collect_blocks_dominated_by(
    cfg: &MethodCfg,
    dominator: BlockId,
    start: BlockId,
) -> Vec<BlockId> {
    let mut result = Vec::new();
    let mut visited = BitSet::new();
    collect_while_dominates(cfg, dominator, start, &mut result, &mut visited);
    result
}

fn collect_while_dominates(
    cfg: &MethodCfg,
    dominator: BlockId,
    child: BlockId,
    result: &mut Vec<BlockId>,
    visited: &mut BitSet,
) {
    let p = cfg.block(child).pos;
    if visited.get(p) {
        return;
    }
    visited.set(p);
    for node in cs(cfg, child) {
        if is_dominator(cfg, node, dominator) {
            result.push(node);
            collect_while_dominates(cfg, dominator, node, result, visited);
        }
    }
}

/// `BlockUtils.visitBlocksOnPath`: the blocks of one path from `start` to `end` (clean successors).
pub fn blocks_on_path(cfg: &MethodCfg, start: BlockId, end: BlockId) -> Vec<BlockId> {
    let mut out = vec![start];
    if start == end {
        return out;
    }
    if cs(cfg, start).contains(&end) {
        out.push(end);
        return out;
    }
    let mut visited = BitSet::new();
    let mut queue: Vec<BlockId> = vec![start];
    loop {
        let Some(&current) = queue.last() else {
            return out_of_path(out);
        };
        let mut added = false;
        for next in cs(cfg, current) {
            if next == end {
                // `queue.removeFirst()`: the start block is already visited
                queue.remove(0);
                queue.push(next);
                let mut res = vec![start];
                res.extend(queue.iter().copied());
                return res;
            }
            let id = cfg.block(next).pos;
            if !visited.get(id) {
                visited.set(id);
                queue.push(next);
                added = true;
                break;
            }
        }
        if !added {
            queue.pop();
            if queue.is_empty() {
                return out_of_path(out);
            }
        }
    }
}

/// The reference's `visitBlocksOnPath` reports only the start block when no path exists.
fn out_of_path(out: Vec<BlockId>) -> Vec<BlockId> {
    out
}

/// Equality of arguments as in `InsnArg.equals`.
pub fn arg_equals(cfg: &MethodCfg, a: &InsnArg, b: &InsnArg) -> bool {
    match (a, b) {
        (InsnArg::Register(x), InsnArg::Register(y)) => x.reg_num == y.reg_num && x.svar == y.svar,
        (InsnArg::Literal(x), InsnArg::Literal(y)) => x.literal == y.literal && x.ty == y.ty,
        (InsnArg::Wrapped(x), InsnArg::Wrapped(y)) => insn_deep_equals(cfg, x.insn, y.insn),
        (InsnArg::Named(x), InsnArg::Named(y)) => x == y,
        _ => false,
    }
}

/// `InsnNode.isSame`
pub fn insn_is_same(cfg: &MethodCfg, a: InsnId, b: InsnId) -> bool {
    if a == b {
        return true;
    }
    let (x, y): (&InsnNode, &InsnNode) = (cfg.insn(a), cfg.insn(b));
    if x.ty != y.ty || x.args.len() != y.args.len() {
        return false;
    }
    for (ax, ay) in x.args.iter().zip(y.args.iter()) {
        if let InsnArg::Wrapped(wx) = ax {
            let InsnArg::Wrapped(wy) = ay else {
                return false;
            };
            if !insn_is_same(cfg, wx.insn, wy.insn) {
                return false;
            }
        }
    }
    // instruction specific parameters of the subclasses
    same_kind(&x.kind, &y.kind)
}

fn same_kind(a: &InsnKind, b: &InsnKind) -> bool {
    match (a, b) {
        (InsnKind::If { op: x, .. }, InsnKind::If { op: y, .. }) => x == y,
        (InsnKind::Arith(x), InsnKind::Arith(y)) => x == y,
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
        (InsnKind::ConstStr(x), InsnKind::ConstStr(y)) => x == y,
        (InsnKind::ConstClass(x), InsnKind::ConstClass(y)) => x == y,
        (InsnKind::Index(x), InsnKind::Index(y)) => x == y,
        (InsnKind::NewArray(x), InsnKind::NewArray(y)) => x == y,
        (InsnKind::FilledNewArray(x), InsnKind::FilledNewArray(y)) => x == y,
        (InsnKind::Goto { .. }, InsnKind::Goto { .. }) => true,
        (InsnKind::None, InsnKind::None) => true,
        (
            InsnKind::Constructor {
                call_mth: m1,
                call_type: t1,
            },
            InsnKind::Constructor {
                call_mth: m2,
                call_type: t2,
            },
        ) => m1 == m2 && t1 == t2,
        _ => std::mem::discriminant(a) == std::mem::discriminant(b),
    }
}

/// `InsnNode.isDeepEquals`
pub fn insn_deep_equals(cfg: &MethodCfg, a: InsnId, b: InsnId) -> bool {
    if a == b {
        return true;
    }
    if !insn_is_same(cfg, a, b) {
        return false;
    }
    let (x, y) = (cfg.insn(a), cfg.insn(b));
    let res_eq = match (&x.result, &y.result) {
        (None, None) => true,
        (Some(r1), Some(r2)) => r1.reg_num == r2.reg_num && r1.svar == r2.svar,
        _ => false,
    };
    res_eq
        && x.args
            .iter()
            .zip(y.args.iter())
            .all(|(p, q)| arg_equals(cfg, p, q))
}

fn is_const_arg(cfg: &MethodCfg, a: &InsnArg) -> bool {
    match a {
        InsnArg::Literal(_) => true,
        InsnArg::Wrapped(w) => crate::passes::insn_utils::is_const_insn(cfg.insn(w.insn).ty),
        _ => false,
    }
}

/// `BlockUtils.isEqualReturnBlocks`
pub fn is_equal_return_blocks(cfg: &MethodCfg, b1: BlockId, b2: BlockId) -> bool {
    if !cfg.block(b1).contains(bflag::RETURN) || !cfg.block(b2).contains(bflag::RETURN) {
        return false;
    }
    let (i1, i2) = (&cfg.block(b1).insns, &cfg.block(b2).insns);
    if i1.len() != 1 || i2.len() != 1 {
        return false;
    }
    let (n1, n2) = (cfg.insn(i1[0]), cfg.insn(i2[0]));
    if n1.args.len() != n2.args.len() {
        return false;
    }
    if n1.args.is_empty() {
        return true;
    }
    let (a, b) = (&n1.args[0], &n2.args[0]);
    if is_const_arg(cfg, a) && is_const_arg(cfg, b) && arg_equals(cfg, a, b) {
        return true;
    }
    // source lines are not tracked
    arg_equals(cfg, a, b)
}

/// `BlockUtils.isSameInsnsBlocks`
pub fn is_same_insns_blocks(cfg: &MethodCfg, first: BlockId, second: BlockId) -> bool {
    let (a, b) = (&cfg.block(first).insns, &cfg.block(second).insns);
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| insn_deep_equals(cfg, *x, *y))
}

/// `BlockUtils.isDuplicateBlockPath`
pub fn is_duplicate_block_path(cfg: &MethodCfg, first: BlockId, second: BlockId) -> bool {
    let (s1, s2) = (&cfg.block(first).succs, &cfg.block(second).succs);
    if s1.len() == 1 && s2.len() == 1 && s1[0] == s2[0] {
        return is_same_insns_blocks(cfg, first, second);
    }
    false
}

fn is_empty_synthetic_path(cfg: &MethodCfg, b1: BlockId, b2: BlockId) -> bool {
    let n1 = follow_empty(cfg, b1);
    let n2 = follow_empty(cfg, b2);
    n1 == n2 || is_equal_return_blocks(cfg, n1, n2)
}

/// `BlockUtils.isEqualPaths`
pub fn is_equal_paths(cfg: &MethodCfg, b1: Option<BlockId>, b2: Option<BlockId>) -> bool {
    if b1 == b2 {
        return true;
    }
    let (Some(b1), Some(b2)) = (b1, b2) else {
        return false;
    };
    is_equal_return_blocks(cfg, b1, b2)
        || is_empty_synthetic_path(cfg, b1, b2)
        || is_duplicate_block_path(cfg, b1, b2)
}

/// `BlockUtils.getPathCross(mth, b1, b2)` with null handling.
pub fn get_path_cross2(
    cfg: &MethodCfg,
    b1: Option<BlockId>,
    b2: Option<BlockId>,
) -> Option<BlockId> {
    match (b1, b2) {
        (Some(x), Some(y)) => crate::passes::block_utils::get_path_cross_pair(cfg, x, y),
        _ => None,
    }
}

/// Iteration order of a `HashSet<BlockNode>` filled with `items` in this order.
pub fn java_hash_order(items: &[BlockId]) -> Vec<BlockId> {
    let mut set = crate::model::bitset::JavaIntSet::new();
    for i in items {
        set.insert(*i);
    }
    set.iter()
}

fn add_predecessors(cfg: &MethodCfg, set: &mut Vec<BlockId>, from: BlockId, until: BlockId) {
    if !set.contains(&from) {
        set.push(from);
    }
    for &pred in &cfg.block(from).preds {
        if pred != until && !set.contains(&pred) {
            add_predecessors(cfg, set, pred, until);
        }
    }
}

/// `BlockUtils.getAllPathsBlocks` in `HashSet` iteration order.
pub fn all_paths_blocks_ordered(cfg: &MethodCfg, start: BlockId, end: BlockId) -> Vec<BlockId> {
    let mut v = vec![start];
    if start != end {
        add_predecessors(cfg, &mut v, end, start);
    }
    java_hash_order(&v)
}
